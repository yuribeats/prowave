//! The one list of control methods: each with its parameters, what it returns and the function
//! that runs it. `rpc.describe`, the checks on a request's parameters, the method table in
//! `docs/CONTROL-API.md` and the list built into `prowavectl` are all made from it, so they
//! cannot say different things.
use super::params::P;
use super::protocol::{Code, RpcError, PROTOCOL};
use super::Cx;
use serde_json::{json, Map, Value};
use std::sync::OnceLock;

/// The JSON type of a parameter.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Ty {
    Str,
    Num,
    Int,
    Bool,
    Arr,
    Any,
}

impl Ty {
    pub fn name(self) -> &'static str {
        match self {
            Ty::Str => "string",
            Ty::Num => "number",
            Ty::Int => "integer",
            Ty::Bool => "boolean",
            Ty::Arr => "array",
            Ty::Any => "any",
        }
    }

    fn fits(self, v: &Value) -> bool {
        match self {
            Ty::Str => v.is_string(),
            Ty::Num => v.as_f64().is_some_and(f64::is_finite),
            Ty::Int => v.as_i64().is_some() || v.as_f64().is_some_and(|f| f.fract() == 0.0),
            Ty::Bool => v.is_boolean(),
            Ty::Arr => v.is_array(),
            Ty::Any => true,
        }
    }
}

/// One named parameter of a method.
#[derive(Clone, Copy, Debug)]
pub struct Param {
    pub name: &'static str,
    pub ty: Ty,
    pub required: bool,
    pub desc: &'static str,
    pub unit: &'static str,
    /// The values a string parameter may have (empty: any).
    pub values: &'static [&'static str],
    pub range: Option<(f64, f64)>,
}

/// A parameter that may be left out.
pub const fn p(name: &'static str, ty: Ty, desc: &'static str) -> Param {
    Param {
        name,
        ty,
        required: false,
        desc,
        unit: "",
        values: &[],
        range: None,
    }
}

impl Param {
    pub const fn req(mut self) -> Self {
        self.required = true;
        self
    }
    pub const fn unit(mut self, unit: &'static str) -> Self {
        self.unit = unit;
        self
    }
    pub const fn vals(mut self, values: &'static [&'static str]) -> Self {
        self.values = values;
        self
    }
    pub const fn range(mut self, lo: f64, hi: f64) -> Self {
        self.range = Some((lo, hi));
        self
    }
}

/// What a method hands back: its result, or nothing yet (it answers the request itself once
/// the work is done, as `ui.screenshot` does when the picture arrives).
pub enum Reply {
    Now(Value),
    Later,
}

pub type Handler = fn(&mut Cx, &P) -> Result<Reply, RpcError>;

pub struct Method {
    pub name: &'static str,
    pub summary: &'static str,
    pub params: Vec<Param>,
    pub result: &'static str,
    /// Changes the project (or replaces it): refused while a question is open in the app,
    /// and followed by an Undo step.
    pub changes: bool,
    pub run: Handler,
}

/// A method that reads.
pub fn m(
    name: &'static str,
    summary: &'static str,
    params: Vec<Param>,
    result: &'static str,
    run: Handler,
) -> Method {
    Method {
        name,
        summary,
        params,
        result,
        changes: false,
        run,
    }
}

impl Method {
    /// The method changes the project.
    pub fn changes(mut self) -> Self {
        self.changes = true;
        self
    }

    /// The part of the name before the first dot.
    pub fn group(&self) -> &'static str {
        self.name.split('.').next().unwrap_or(self.name)
    }

    fn to_json(&self) -> Value {
        let params: Vec<Value> = self
            .params
            .iter()
            .map(|p| {
                let mut o = Map::new();
                o.insert("name".into(), p.name.into());
                o.insert("type".into(), p.ty.name().into());
                o.insert("required".into(), p.required.into());
                o.insert("description".into(), p.desc.into());
                if !p.unit.is_empty() {
                    o.insert("unit".into(), p.unit.into());
                }
                if !p.values.is_empty() {
                    o.insert("values".into(), json!(p.values));
                }
                if let Some((lo, hi)) = p.range {
                    o.insert("min".into(), json!(lo));
                    o.insert("max".into(), json!(hi));
                }
                Value::Object(o)
            })
            .collect();
        json!({
            "name": self.name,
            "group": self.group(),
            "summary": self.summary,
            "changes_project": self.changes,
            "params": params,
            "result": self.result,
        })
    }
}

/// Every method, in the order the documents list them.
pub fn all() -> &'static [Method] {
    static ALL: OnceLock<Vec<Method>> = OnceLock::new();
    ALL.get_or_init(|| {
        let mut v = Vec::new();
        v.extend(super::m_core::methods());
        v.extend(super::m_tracks::methods());
        v.extend(super::m_midi_fx::methods());
        v.extend(super::m_kits::methods());
        v.extend(super::m_rack::methods());
        v.extend(super::m_rack_follow::methods());
        v.extend(super::m_regions::methods());
        v.extend(super::m_ui::methods());
        v.extend(super::render::methods());
        v
    })
}

pub fn find(name: &str) -> Option<&'static Method> {
    all().iter().find(|m| m.name == name)
}

/// The error for a method name that is not in the list, with the nearest names.
pub fn unknown(name: &str) -> RpcError {
    let group = name.split('.').next().unwrap_or("");
    let mut near: Vec<&str> = all()
        .iter()
        .filter(|m| m.group() == group || m.name.contains(name) || name.contains(m.name))
        .map(|m| m.name)
        .collect();
    near.truncate(40);
    let hint = if near.is_empty() {
        "rpc.describe lists every method".to_string()
    } else {
        format!("did you mean one of: {}", near.join(", "))
    };
    RpcError::new(
        Code::MethodNotFound,
        format!("there is no method `{name}`; {hint}"),
    )
}

/// Checks `params` against the method's list: no unknown name, none missing, each of its type,
/// one of its values and inside its range.
pub fn validate(method: &Method, params: &Map<String, Value>) -> Result<(), RpcError> {
    let names = || {
        if method.params.is_empty() {
            "it takes no parameters".to_string()
        } else {
            format!(
                "it takes: {}",
                method
                    .params
                    .iter()
                    .map(|p| if p.required {
                        format!("{} (required)", p.name)
                    } else {
                        p.name.to_string()
                    })
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        }
    };
    for key in params.keys() {
        if !method.params.iter().any(|p| p.name == key) {
            return Err(RpcError::invalid(
                key,
                format!("`{}` has no parameter `{key}`; {}", method.name, names()),
            ));
        }
    }
    for p in &method.params {
        let v = match params.get(p.name) {
            None | Some(Value::Null) => {
                if p.required {
                    return Err(RpcError::invalid(
                        p.name,
                        format!(
                            "`{}` needs the parameter `{}` ({}): {}",
                            method.name,
                            p.name,
                            p.ty.name(),
                            p.desc
                        ),
                    ));
                }
                continue;
            }
            Some(v) => v,
        };
        if !p.ty.fits(v) {
            return Err(RpcError::invalid(
                p.name,
                format!(
                    "`{}` must be a {} ({}); got {v}",
                    p.name,
                    p.ty.name(),
                    p.desc
                ),
            ));
        }
        if !p.values.is_empty() {
            if let Some(s) = v.as_str() {
                if !p.values.contains(&s) {
                    return Err(RpcError::invalid(
                        p.name,
                        format!(
                            "`{}` must be one of: {}; got \"{s}\"",
                            p.name,
                            p.values.join(", ")
                        ),
                    )
                    .with("values", json!(p.values)));
                }
            }
        }
        if let (Some((lo, hi)), Some(x)) = (p.range, v.as_f64()) {
            if x < lo || x > hi {
                let unit = if p.unit.is_empty() {
                    String::new()
                } else {
                    format!(" {}", p.unit)
                };
                return Err(RpcError::invalid(
                    p.name,
                    format!("`{}` must be from {lo} to {hi}{unit}; got {x}", p.name),
                )
                .with("min", lo)
                .with("max", hi));
            }
        }
    }
    Ok(())
}

fn errors_json() -> Value {
    Code::ALL
        .iter()
        .map(|c| json!({"name": c.name(), "number": c.number(), "meaning": c.meaning()}))
        .collect()
}

/// What `rpc.describe` returns: every method (or only `only`), and the error codes.
pub fn describe(only: Option<&str>, socket: Option<&std::path::Path>) -> Value {
    let methods: Vec<Value> = all()
        .iter()
        .filter(|m| only.is_none_or(|o| o == m.name))
        .map(Method::to_json)
        .collect();
    let mut out = json!({
        "protocol": PROTOCOL,
        "app": "Pro Wave",
        "version": env!("CARGO_PKG_VERSION"),
        "commit": option_env!("PROWAVE_COMMIT").unwrap_or(""),
        "methods": methods,
        "errors": errors_json(),
    });
    if let (Some(s), Some(o)) = (socket, out.as_object_mut()) {
        o.insert("socket".into(), s.display().to_string().into());
    }
    out
}

#[cfg(test)]
/// The list `prowavectl` carries for when the app is not running: [`describe`] without what
/// only a running app knows (its commit and socket), so the file changes only with the list.
pub fn embedded_list() -> String {
    let mut v = describe(None, None);
    if let Some(o) = v.as_object_mut() {
        o.insert("commit".into(), "".into());
    }
    let mut text = serde_json::to_string_pretty(&v).unwrap_or_default();
    text.push('\n');
    text
}

#[cfg(test)]
/// The method table of `docs/CONTROL-API.md`: one section a group, one line a method.
pub fn markdown() -> String {
    let mut out = String::new();
    let mut groups: Vec<&str> = Vec::new();
    for m in all() {
        if !groups.contains(&m.group()) {
            groups.push(m.group());
        }
    }
    out.push_str(&format!(
        "{} methods in {} groups. A parameter in **bold** is required.\n",
        all().len(),
        groups.len()
    ));
    for g in groups {
        out.push_str(&format!("\n### {g}\n\n"));
        out.push_str("| Method | What it does | Parameters | Result |\n|---|---|---|---|\n");
        for m in all().iter().filter(|m| m.group() == g) {
            let params = if m.params.is_empty() {
                "none".to_string()
            } else {
                m.params
                    .iter()
                    .map(|p| {
                        let name = if p.required {
                            format!("**{}**", p.name)
                        } else {
                            p.name.to_string()
                        };
                        let mut extra = p.ty.name().to_string();
                        if !p.unit.is_empty() {
                            extra.push_str(&format!(", {}", p.unit));
                        }
                        if let Some((lo, hi)) = p.range {
                            extra.push_str(&format!(", {lo} to {hi}"));
                        }
                        if !p.values.is_empty() {
                            extra.push_str(&format!(": {}", p.values.join(" / ")));
                        }
                        format!("{name} ({extra}): {}", p.desc)
                    })
                    .collect::<Vec<_>>()
                    .join("<br>")
            };
            let cell = |s: &str| s.replace('|', "\\|");
            out.push_str(&format!(
                "| `{}` | {} | {} | {} |\n",
                m.name,
                cell(m.summary),
                cell(&params),
                cell(m.result)
            ));
        }
    }
    out
}
