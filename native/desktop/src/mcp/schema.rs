//! JSON Schemas of tool arguments: small builders for the tool table and a
//! check of arguments against the subset of JSON Schema they use.

use serde_json::{json, Map, Value};

/// One argument of a tool.
pub struct Argument {
    pub name: &'static str,
    pub required: bool,
    pub schema: Value,
}

pub fn required(name: &'static str, schema: Value) -> Argument {
    Argument {
        name,
        required: true,
        schema,
    }
}

pub fn optional(name: &'static str, schema: Value) -> Argument {
    Argument {
        name,
        required: false,
        schema,
    }
}

/// The schema of an argument object that allows exactly these arguments.
pub fn object(arguments: Vec<Argument>) -> Value {
    let mut properties = Map::new();
    let mut names = Vec::new();
    for argument in arguments {
        if argument.required {
            names.push(Value::from(argument.name));
        }
        properties.insert(argument.name.to_owned(), argument.schema);
    }
    let mut schema = json!({
        "type": "object",
        "properties": properties,
        "additionalProperties": false,
    });
    if !names.is_empty() {
        schema["required"] = Value::Array(names);
    }
    schema
}

pub fn number_in(description: &str, minimum: f64, maximum: f64) -> Value {
    json!({"type": "number", "minimum": minimum, "maximum": maximum, "description": description})
}

/// A number with a lower limit only.
pub fn number_from(description: &str, minimum: f64) -> Value {
    json!({"type": "number", "minimum": minimum, "description": description})
}

pub fn positive(description: &str) -> Value {
    json!({"type": "number", "exclusiveMinimum": 0, "description": description})
}

/// A number within limits, or `null` to leave the value to the window.
pub fn number_or_null(description: &str, minimum: f64, maximum: f64) -> Value {
    json!({
        "type": ["number", "null"],
        "minimum": minimum,
        "maximum": maximum,
        "description": description,
    })
}

/// A number above zero with an upper limit.
pub fn positive_up_to(description: &str, maximum: f64) -> Value {
    json!({"type": "number", "exclusiveMinimum": 0, "maximum": maximum, "description": description})
}

pub fn integer_in(description: &str, minimum: u64, maximum: u64) -> Value {
    json!({"type": "integer", "minimum": minimum, "maximum": maximum, "description": description})
}

/// A zero-based place in a list.
pub fn ordinal(description: &str) -> Value {
    json!({"type": "integer", "minimum": 0, "description": description})
}

pub fn boolean(description: &str) -> Value {
    json!({"type": "boolean", "description": description})
}

pub fn text(description: &str, min_length: usize, max_length: usize) -> Value {
    json!({
        "type": "string",
        "minLength": min_length,
        "maxLength": max_length,
        "description": description,
    })
}

pub fn choice(description: &str, values: &[&str]) -> Value {
    json!({"type": "string", "enum": values, "description": description})
}

/// An absolute file or folder path on the computer running the window.
pub fn path(description: &str) -> Value {
    json!({"type": "string", "minLength": 1, "description": description})
}

/// A position `[x, y, z]` in scene coordinates.
pub fn xyz(description: &str) -> Value {
    numbers(description, 3)
}

/// A position `[x, y, z]` in scene coordinates, or `null`.
pub fn xyz_or_null(description: &str) -> Value {
    let mut schema = xyz(description);
    schema["type"] = json!(["array", "null"]);
    schema
}

/// A position `[x, y]` in viewport pixels.
pub fn pixel(description: &str) -> Value {
    numbers(description, 2)
}

pub fn numbers(description: &str, count: usize) -> Value {
    json!({
        "type": "array",
        "items": {"type": "number"},
        "minItems": count,
        "maxItems": count,
        "description": description,
    })
}

pub fn list(description: &str, items: Value, min_items: usize, max_items: usize) -> Value {
    json!({
        "type": "array",
        "items": items,
        "minItems": min_items,
        "maxItems": max_items,
        "description": description,
    })
}

/// A number without a fractional part, such as `17.0`, as the integer it
/// is; JSON Schema counts it as an integer. Only numbers that a 64-bit float
/// holds exactly qualify.
fn whole(value: &Value) -> Option<i64> {
    const EXACT: f64 = 9_007_199_254_740_992.0;
    let number = value.as_f64().filter(|_| value.is_f64())?;
    (number.fract() == 0.0 && number.abs() <= EXACT).then_some(number as i64)
}

fn has_type(value: &Value, name: &str) -> bool {
    match name {
        "object" => value.is_object(),
        "array" => value.is_array(),
        "string" => value.is_string(),
        "boolean" => value.is_boolean(),
        "null" => value.is_null(),
        "number" => value.is_number(),
        "integer" => value.is_i64() || value.is_u64() || whole(value).is_some(),
        _ => false,
    }
}

/// Write the whole numbers that `schema` asks an integer for as integers,
/// such as `17.0` as `17`, so the command API reads them. Call it on
/// arguments that passed `validate_arguments`.
pub fn whole_numbers_as_integers(schema: &Value, value: &mut Value) {
    let integer = match &schema["type"] {
        Value::String(name) => name == "integer",
        Value::Array(names) => names.iter().any(|name| name == "integer"),
        _ => false,
    };
    if integer {
        if let Some(number) = whole(value) {
            *value = Value::from(number);
            return;
        }
    }
    match value {
        Value::Array(items) => {
            for item in items {
                whole_numbers_as_integers(&schema["items"], item);
            }
        }
        Value::Object(fields) => {
            for (name, field) in fields {
                whole_numbers_as_integers(&schema["properties"][name.as_str()], field);
            }
        }
        _ => {}
    }
}

fn describe(value: &Value) -> String {
    let text = value.to_string();
    if text.chars().count() > 40 {
        let cut: String = text.chars().take(39).collect();
        format!("{cut}…")
    } else {
        text
    }
}

/// Check the arguments of a tool call against the tool's schema.
pub fn validate_arguments(schema: &Value, arguments: &Value) -> Result<(), String> {
    if !arguments.is_object() {
        return Err(format!(
            "arguments must be an object, not {}",
            describe(arguments)
        ));
    }
    validate(schema, arguments, "")
}

/// Check a value against a schema built from the keywords above: `type`,
/// `enum`, the numeric and length limits, `items`, `properties`, `required`,
/// `minProperties` and `additionalProperties: false`. `at` names the value in messages; it
/// is empty for the arguments object itself.
fn validate(schema: &Value, value: &Value, at: &str) -> Result<(), String> {
    if let Some(expected) = schema.get("type") {
        let names: Vec<&str> = match expected {
            Value::String(name) => vec![name.as_str()],
            Value::Array(names) => names.iter().filter_map(Value::as_str).collect(),
            _ => Vec::new(),
        };
        if !names.iter().any(|name| has_type(value, name)) {
            let article = |name: &str| match name {
                "array" | "integer" | "object" => format!("an {name}"),
                "null" => "null".to_owned(),
                _ => format!("a {name}"),
            };
            let wanted: Vec<String> = names.iter().map(|name| article(name)).collect();
            return Err(format!(
                "{at} must be {}, not {}",
                wanted.join(" or "),
                describe(value)
            ));
        }
    }
    if let Some(allowed) = schema.get("enum").and_then(Value::as_array) {
        if !allowed.contains(value) {
            let names: Vec<String> = allowed.iter().map(Value::to_string).collect();
            return Err(format!("{at} must be one of {}", names.join(", ")));
        }
    }
    if let Some(number) = value.as_f64() {
        if let Some(minimum) = schema.get("minimum").and_then(Value::as_f64) {
            if number < minimum {
                return Err(format!("{at} must be at least {minimum}"));
            }
        }
        if let Some(maximum) = schema.get("maximum").and_then(Value::as_f64) {
            if number > maximum {
                return Err(format!("{at} must be at most {maximum}"));
            }
        }
        if let Some(minimum) = schema.get("exclusiveMinimum").and_then(Value::as_f64) {
            if number <= minimum {
                return Err(format!("{at} must be greater than {minimum}"));
            }
        }
    }
    if let Some(text) = value.as_str() {
        let length = text.chars().count() as u64;
        if let Some(minimum) = schema.get("minLength").and_then(Value::as_u64) {
            if length < minimum {
                return Err(format!("{at} must have at least {minimum} characters"));
            }
        }
        if let Some(maximum) = schema.get("maxLength").and_then(Value::as_u64) {
            if length > maximum {
                return Err(format!("{at} must have at most {maximum} characters"));
            }
        }
    }
    if let Some(items) = value.as_array() {
        let count = items.len() as u64;
        if let Some(minimum) = schema.get("minItems").and_then(Value::as_u64) {
            if count < minimum {
                return Err(format!("{at} must have at least {minimum} items"));
            }
        }
        if let Some(maximum) = schema.get("maxItems").and_then(Value::as_u64) {
            if count > maximum {
                return Err(format!("{at} must have at most {maximum} items"));
            }
        }
        if let Some(item_schema) = schema.get("items") {
            for (place, item) in items.iter().enumerate() {
                validate(item_schema, item, &format!("{at}[{place}]"))?;
            }
        }
    }
    if let Some(fields) = value.as_object() {
        let properties = schema.get("properties").and_then(Value::as_object);
        let nested = |name: &str| {
            if at.is_empty() {
                name.to_owned()
            } else {
                format!("{at}.{name}")
            }
        };
        for name in schema
            .get("required")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
        {
            if !fields.contains_key(name) {
                return Err(format!("missing required argument {}", nested(name)));
            }
        }
        if let Some(minimum) = schema.get("minProperties").and_then(Value::as_u64) {
            if (fields.len() as u64) < minimum {
                let names: Vec<String> = properties
                    .into_iter()
                    .flat_map(|properties| properties.keys())
                    .map(|name| nested(name))
                    .collect();
                return Err(format!(
                    "give at least {minimum} of the arguments {}",
                    names.join(", ")
                ));
            }
        }
        for (name, field) in fields {
            match properties.and_then(|properties| properties.get(name)) {
                Some(field_schema) => validate(field_schema, field, &nested(name))?,
                None if schema.get("additionalProperties") == Some(&Value::Bool(false)) => {
                    return Err(format!("unknown argument {}", nested(name)));
                }
                None => {}
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn camera() -> Value {
        object(vec![
            required("yaw", number_in("Heading", -3.2, 3.2)),
            required("pan", pixel("Shift")),
            optional("name", text("Name", 1, 4)),
            optional(
                "tool",
                json!({"type": ["string", "null"], "enum": ["note", null]}),
            ),
            optional("count", integer_in("Count", 1, 9)),
            optional("factor", positive("Factor")),
        ])
    }

    #[test]
    fn builders_make_closed_object_schemas() {
        let schema = camera();
        assert_eq!(schema["type"], "object");
        assert_eq!(schema["additionalProperties"], false);
        assert_eq!(schema["required"], json!(["yaw", "pan"]));
        assert_eq!(schema["properties"]["pan"]["minItems"], 2);
        assert!(object(Vec::new()).get("required").is_none());
    }

    #[test]
    fn validation_reports_the_first_problem_by_name() {
        let schema = camera();
        let check = |value: Value| validate_arguments(&schema, &value);
        assert_eq!(check(json!({"yaw": 0.5, "pan": [1, 2.5]})), Ok(()));
        assert_eq!(
            check(json!({"yaw": 0.5, "pan": [1, 2], "tool": null, "count": 3})),
            Ok(())
        );
        assert_eq!(
            check(json!({"pan": [1, 2]})),
            Err("missing required argument yaw".into())
        );
        assert_eq!(
            check(json!({"yaw": "0.5", "pan": [1, 2]})),
            Err("yaw must be a number, not \"0.5\"".into())
        );
        assert_eq!(
            check(json!({"yaw": 4, "pan": [1, 2]})),
            Err("yaw must be at most 3.2".into())
        );
        assert_eq!(
            check(json!({"yaw": 0, "pan": [1]})),
            Err("pan must have at least 2 items".into())
        );
        assert_eq!(
            check(json!({"yaw": 0, "pan": [1, "x"]})),
            Err("pan[1] must be a number, not \"x\"".into())
        );
        assert_eq!(
            check(json!({"yaw": 0, "pan": [1, 2], "zoom": 1})),
            Err("unknown argument zoom".into())
        );
        assert_eq!(
            check(json!({"yaw": 0, "pan": [1, 2], "name": "toolong"})),
            Err("name must have at most 4 characters".into())
        );
        assert_eq!(
            check(json!({"yaw": 0, "pan": [1, 2], "tool": "line"})),
            Err("tool must be one of \"note\", null".into())
        );
        assert_eq!(
            check(json!({"yaw": 0, "pan": [1, 2], "count": 2.5})),
            Err("count must be an integer, not 2.5".into())
        );
        assert_eq!(
            check(json!({"yaw": 0, "pan": [1, 2], "factor": 0})),
            Err("factor must be greater than 0".into())
        );
        assert_eq!(
            check(json!([1])),
            Err("arguments must be an object, not [1]".into())
        );
        assert_eq!(
            check(json!({"yaw": 0, "pan": [1, 2], "count": 1e300})),
            Err("count must be an integer, not 1e+300".into())
        );
    }

    #[test]
    fn whole_numbers_count_as_integers_and_are_sent_as_integers() {
        let schema = camera();
        let mut arguments = json!({"yaw": 1.0, "pan": [3.0, 2.5], "count": 3.0, "factor": 2.0});
        assert_eq!(validate_arguments(&schema, &arguments), Ok(()));
        whole_numbers_as_integers(&schema, &mut arguments);
        assert_eq!(
            arguments.to_string(),
            r#"{"count":3,"factor":2.0,"pan":[3.0,2.5],"yaw":1.0}"#
        );

        let mut one_of = object(vec![
            optional("pid", integer_in("Process", 1, 9)),
            optional("port", integer_in("Port", 1, 9)),
        ]);
        one_of["minProperties"] = json!(1);
        assert_eq!(
            validate_arguments(&one_of, &json!({})),
            Err("give at least 1 of the arguments pid, port".into())
        );
        assert_eq!(validate_arguments(&one_of, &json!({"port": 4.0})), Ok(()));
    }
}
