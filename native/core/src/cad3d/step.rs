//! The pieces of a STEP physical file (ISO 10303-21) that the IFC writer
//! needs: reals, strings, globally unique ids and the time stamp; and, for
//! the tests, a reader that checks what was written.

use std::time::{SystemTime, UNIX_EPOCH};

/// A real as STEP writes it: always with a decimal point, and an exponent
/// with a capital E.
pub(crate) fn real(value: f64) -> String {
    if value == 0.0 || !value.is_finite() {
        return "0.".into();
    }
    let text = format!("{value:?}");
    match text.split_once('e') {
        Some((mantissa, exponent)) if mantissa.contains('.') => format!("{mantissa}E{exponent}"),
        Some((mantissa, exponent)) => format!("{mantissa}.E{exponent}"),
        None if text.contains('.') => text,
        None => format!("{text}."),
    }
}

/// A length to the micrometre: what a scan holds and no more digits.
pub(crate) fn length(value: f64) -> String {
    real((value * 1e6).round() / 1e6)
}

/// A string as STEP writes it: between single quotes, a quote and a
/// backslash doubled, and every character outside printable ASCII as its
/// UTF-16 units in a `\X2\` ... `\X0\` run. Control characters are left
/// out.
pub(crate) fn string(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('\'');
    let mut wide = String::new();
    let flush = |wide: &mut String, out: &mut String| {
        if !wide.is_empty() {
            out.push_str("\\X2\\");
            out.push_str(wide);
            out.push_str("\\X0\\");
            wide.clear();
        }
    };
    for character in value.chars() {
        if character.is_control() {
            continue;
        }
        if character.is_ascii() {
            flush(&mut wide, &mut out);
            match character {
                '\'' => out.push_str("''"),
                '\\' => out.push_str("\\\\"),
                other => out.push(other),
            }
        } else {
            let mut units = [0u16; 2];
            for unit in character.encode_utf16(&mut units) {
                wide.push_str(&format!("{unit:04X}"));
            }
        }
    }
    flush(&mut wide, &mut out);
    out.push('\'');
    out
}

const GUID_CHARACTERS: &[u8; 64] =
    b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz_$";

/// A new globally unique id in the 22 characters of IFC: the 128 bits of a
/// random UUID, two in the first character and six in each other one.
pub(crate) fn guid() -> String {
    compress_guid(uuid::Uuid::new_v4().as_u128())
}

pub(crate) fn compress_guid(value: u128) -> String {
    (0..22)
        .map(|position| {
            let shift = 6 * (21 - position);
            let bits = (value >> shift) & if position == 0 { 0b11 } else { 0b11_1111 };
            char::from(GUID_CHARACTERS[bits as usize])
        })
        .collect()
}

/// The present time as `YYYY-MM-DDThh:mm:ss`, in UTC.
pub(crate) fn time_stamp() -> String {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs());
    format_time(seconds)
}

pub(crate) fn format_time(seconds: u64) -> String {
    let days = (seconds / 86_400) as i64;
    let rest = seconds % 86_400;
    // Days since 1970-01-01 to a civil date (proleptic Gregorian).
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let of_era = shifted.rem_euclid(146_097);
    let year_of_era = (of_era - of_era / 1_460 + of_era / 36_524 - of_era / 146_096) / 365;
    let day_of_year = of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}",
        rest / 3_600,
        rest / 60 % 60,
        rest % 60
    )
}

#[cfg(test)]
pub(crate) mod check {
    //! A strict reader of the files the writer makes, for the tests.

    use std::collections::BTreeMap;

    /// One instance of the DATA section: its type and its attributes, each
    /// as the text it was written as.
    #[derive(Debug, Clone)]
    pub(crate) struct Instance {
        pub name: String,
        pub attributes: Vec<String>,
    }

    /// Split a parenthesised list into its members at the top level.
    pub(crate) fn members(list: &str) -> Result<Vec<String>, String> {
        let inner = list
            .strip_prefix('(')
            .and_then(|rest| rest.strip_suffix(')'))
            .ok_or_else(|| format!("not a list: {list}"))?;
        let mut parts = Vec::new();
        let mut depth = 0i32;
        let mut quoted = false;
        let mut current = String::new();
        let mut characters = inner.chars().peekable();
        while let Some(character) = characters.next() {
            if quoted {
                current.push(character);
                if character == '\'' {
                    if characters.peek() == Some(&'\'') {
                        current.push(characters.next().unwrap());
                    } else {
                        quoted = false;
                    }
                }
                continue;
            }
            match character {
                '\'' => {
                    quoted = true;
                    current.push(character);
                }
                '(' => {
                    depth += 1;
                    current.push(character);
                }
                ')' => {
                    depth -= 1;
                    if depth < 0 {
                        return Err(format!("unbalanced: {list}"));
                    }
                    current.push(character);
                }
                ',' if depth == 0 => parts.push(std::mem::take(&mut current)),
                other => current.push(other),
            }
        }
        if quoted || depth != 0 {
            return Err(format!("unbalanced: {list}"));
        }
        if !current.is_empty() || !parts.is_empty() {
            parts.push(current);
        }
        Ok(parts
            .into_iter()
            .map(|part| part.trim().to_owned())
            .collect())
    }

    /// Every `#n` in a text, outside strings.
    pub(crate) fn references(text: &str) -> Vec<u64> {
        let mut found = Vec::new();
        let mut quoted = false;
        let bytes = text.as_bytes();
        let mut index = 0;
        while index < bytes.len() {
            match bytes[index] {
                b'\'' => quoted = !quoted,
                b'#' if !quoted => {
                    let digits: String = text[index + 1..]
                        .chars()
                        .take_while(char::is_ascii_digit)
                        .collect();
                    if let Ok(id) = digits.parse() {
                        found.push(id);
                    }
                    index += digits.len();
                }
                _ => {}
            }
            index += 1;
        }
        found
    }

    /// Read a file: its header names IFC4, every instance is `#n=NAME(...);`
    /// with balanced brackets and strings, every number is used once, every
    /// reference points to an instance, and every instance has the number
    /// of attributes its type has in IFC4. Returns the instances by number.
    pub(crate) fn read(text: &str) -> Result<BTreeMap<u64, Instance>, String> {
        let text = text.replace("\r\n", "\n");
        if !text.starts_with("ISO-10303-21;\n") || !text.trim_end().ends_with("END-ISO-10303-21;") {
            return Err("not a STEP physical file".into());
        }
        if !text.contains("FILE_SCHEMA(('IFC4'));") {
            return Err("the schema is not IFC4".into());
        }
        let data = text
            .split_once("\nDATA;\n")
            .and_then(|(_, rest)| rest.split_once("\nENDSEC;\nEND-ISO-10303-21;"))
            .map(|(data, _)| data)
            .ok_or("no DATA section")?;
        let mut instances = BTreeMap::new();
        for line in data.lines() {
            let line = line.trim();
            let body = line
                .strip_suffix(';')
                .ok_or_else(|| format!("no semicolon: {line}"))?;
            let (number, rest) = body
                .strip_prefix('#')
                .and_then(|rest| rest.split_once('='))
                .ok_or_else(|| format!("not an instance: {line}"))?;
            let number: u64 = number.parse().map_err(|_| format!("bad number: {line}"))?;
            let open = rest
                .find('(')
                .ok_or_else(|| format!("no attributes: {line}"))?;
            let (name, list) = rest.split_at(open);
            if name.is_empty()
                || !name
                    .chars()
                    .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
            {
                return Err(format!("bad type name: {line}"));
            }
            let attributes = members(list)?;
            if let Some(expected) = attribute_count(name) {
                if attributes.len() != expected {
                    return Err(format!(
                        "{name} has {} attributes instead of {expected}: {line}",
                        attributes.len()
                    ));
                }
            } else {
                return Err(format!("unexpected type {name}"));
            }
            let instance = Instance {
                name: name.to_owned(),
                attributes,
            };
            if instances.insert(number, instance).is_some() {
                return Err(format!("#{number} is used twice"));
            }
        }
        for (number, instance) in &instances {
            for attribute in &instance.attributes {
                for reference in references(attribute) {
                    if !instances.contains_key(&reference) {
                        return Err(format!(
                            "#{number} refers to #{reference}, which is missing"
                        ));
                    }
                }
            }
        }
        Ok(instances)
    }

    /// The number of attributes of the IFC4 types the writer uses.
    fn attribute_count(name: &str) -> Option<usize> {
        Some(match name {
            "IFCPROJECT" => 9,
            "IFCSITE" => 14,
            "IFCBUILDING" => 12,
            "IFCBUILDINGSTOREY" => 10,
            "IFCBUILDINGELEMENTPROXY" => 9,
            "IFCRELAGGREGATES" => 6,
            "IFCRELCONTAINEDINSPATIALSTRUCTURE" => 6,
            "IFCRELDEFINESBYPROPERTIES" => 6,
            "IFCPROPERTYSET" => 5,
            "IFCPROPERTYSINGLEVALUE" => 4,
            "IFCSIUNIT" => 4,
            "IFCUNITASSIGNMENT" => 1,
            "IFCGEOMETRICREPRESENTATIONCONTEXT" => 6,
            "IFCGEOMETRICREPRESENTATIONSUBCONTEXT" => 10,
            "IFCCARTESIANPOINT" | "IFCDIRECTION" | "IFCPOLYLINE" => 1,
            "IFCAXIS2PLACEMENT3D" => 3,
            "IFCLOCALPLACEMENT" => 2,
            "IFCPRODUCTDEFINITIONSHAPE" => 3,
            "IFCSHAPEREPRESENTATION" => 4,
            "IFCCARTESIANPOINTLIST3D" => 1,
            "IFCTRIANGULATEDFACESET" => 5,
            "IFCPOLYGONALFACESET" => 4,
            "IFCINDEXEDPOLYGONALFACE" => 1,
            "IFCINDEXEDPOLYGONALFACEWITHVOIDS" => 2,
            "IFCCIRCLEPROFILEDEF" => 4,
            "IFCEXTRUDEDAREASOLID" => 4,
            "IFCSTYLEDITEM" => 3,
            "IFCSURFACESTYLE" => 3,
            "IFCSURFACESTYLESHADING" => 2,
            "IFCCOLOURRGB" => 4,
            _ => return None,
        })
    }
}
