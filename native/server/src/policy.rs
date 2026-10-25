//! What the cloud allows: only the tools of the table, and no argument that
//! names a file on the server.
//!
//! The first line of defence is here: an unknown tool or an argument such
//! as `path` is refused before the arguments are typed or anything is queued.
//! The second line holds where this one would miss something:
//! 1. the argument types of the tools hold no field that is a path (a test
//!    walks every schema for such names and for `format: path`);
//! 2. only `tools::call_blocking` makes paths, inside the fresh work folder
//!    of a call, from fixed names (`input.<format>`, `out/<stem>.<ext>`);
//! 3. the disk caches of the core are off, so the core reads nothing that an
//!    earlier call, or anything else, left under its cache folder.

use std::fmt;

use serde::de::{Deserializer, IgnoredAny, MapAccess, Visitor};

use crate::error::{short, ApiError};
use crate::tools::{self, Tool};

/// Arguments that point at a file on the server: refused in every call.
pub const FORBIDDEN_ARGUMENTS: &[&str] = &[
    "path",
    "project_path",
    "project",
    "pad",
    "output_path",
    "destination",
    "directory",
];

/// The keys of a JSON object, its values skipped unread.
struct Keys(Vec<String>);

impl<'de> serde::Deserialize<'de> for Keys {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct KeyVisitor;

        impl<'de> Visitor<'de> for KeyVisitor {
            type Value = Keys;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a JSON object with the arguments")
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Keys, A::Error> {
                let mut keys = Vec::new();
                while let Some(key) = map.next_key::<String>()? {
                    map.next_value::<IgnoredAny>()?;
                    keys.push(key);
                }
                Ok(Keys(keys))
            }
        }

        deserializer.deserialize_map(KeyVisitor)
    }
}

/// The top-level keys of the arguments `body` holds; anything but one JSON
/// object is refused. Only the keys are kept: the values are checked for
/// their syntax and skipped.
pub fn top_level_keys(body: &[u8]) -> Result<Vec<String>, ApiError> {
    serde_json::from_slice::<Keys>(body)
        .map(|keys| keys.0)
        .map_err(|error| {
            ApiError::argument(format!(
                "The arguments must be one JSON object: {}",
                short(&error)
            ))
        })
}

/// The tool `name`, when the cloud offers it and none of `keys` is a
/// forbidden argument: 404 for an unknown tool, 400 for such an argument.
pub fn check(name: &str, keys: &[String]) -> Result<&'static Tool, ApiError> {
    let tool = tools::find(name).ok_or_else(|| ApiError::unknown_tool(name))?;
    if let Some(forbidden) = keys
        .iter()
        .find(|key| FORBIDDEN_ARGUMENTS.contains(&key.as_str()))
    {
        return Err(ApiError::argument(format!(
            "The argument `{forbidden}` points at a file and is not allowed in the cloud API: send the file itself as input.data."
        )));
    }
    Ok(tool)
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    use super::*;

    #[test]
    fn only_the_keys_of_an_object_are_read() {
        let keys =
            top_level_keys(br#"{"input": {"data": "QUJD", "path": "x"}, "box": [1, 2]}"#).unwrap();
        assert_eq!(keys, ["input", "box"]);
        assert_eq!(top_level_keys(b"{}").unwrap(), Vec::<String>::new());
        for wrong in [&b"[1, 2]"[..], b"\"text\"", b"{\"a\": ", b"", b"{} {}"] {
            let error = top_level_keys(wrong).unwrap_err();
            assert_eq!(error.code, "ARGUMENT_ONGELDIG");
        }
    }

    #[test]
    fn unknown_tools_and_forbidden_arguments_are_refused() {
        let error = check("load_project", &[]).unwrap_err();
        assert_eq!((error.status.as_u16(), error.code), (404, "TOOL_ONBEKEND"));
        for forbidden in FORBIDDEN_ARGUMENTS {
            let error = check("convert", &[(*forbidden).to_owned()]).unwrap_err();
            assert_eq!(
                (error.status.as_u16(), error.code),
                (400, "ARGUMENT_ONGELDIG")
            );
            assert!(error.melding.contains(forbidden));
        }
        assert_eq!(check("convert", &["input".into()]).unwrap().name, "convert");
    }

    /// A forbidden name as a property or in a list of required ones, or a
    /// string of `format: path`, anywhere in a schema.
    fn path_like(schema: &Value) -> Option<String> {
        match schema {
            Value::Object(map) => {
                if let Some(Value::Object(properties)) = map.get("properties") {
                    if let Some(name) = properties.keys().find(|name| {
                        FORBIDDEN_ARGUMENTS.contains(&name.as_str())
                            || name.ends_with("_path")
                            || name.ends_with("_file")
                            || name.ends_with("_dir")
                    }) {
                        return Some(name.clone());
                    }
                }
                if let Some(Value::Array(required)) = map.get("required") {
                    if let Some(name) = required
                        .iter()
                        .filter_map(Value::as_str)
                        .find(|name| FORBIDDEN_ARGUMENTS.contains(name))
                    {
                        return Some(name.to_owned());
                    }
                }
                if map.get("format").and_then(Value::as_str) == Some("path") {
                    return Some("format: path".into());
                }
                map.values().find_map(path_like)
            }
            Value::Array(items) => items.iter().find_map(path_like),
            _ => None,
        }
    }

    #[test]
    fn no_schema_anywhere_holds_a_path() {
        for tool in tools::table() {
            assert_eq!(path_like(&tool.input_schema()), None, "{}", tool.name);
            assert_eq!(path_like(&tool.listing()), None, "{}", tool.name);
        }
        // The walk finds what it looks for.
        let planted =
            serde_json::json!({"properties": {"input": {"properties": {"output_path": {}}}}});
        assert_eq!(path_like(&planted), Some("output_path".into()));
        assert_eq!(
            path_like(&serde_json::json!({"anyOf": [{"type": "string", "format": "path"}]})),
            Some("format: path".into())
        );
    }
}
