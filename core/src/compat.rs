//! Legacy cache compatibility. The Python JSON separators and key ordering are
//! part of the signature format, even though ordinary JSON ignores them.
use serde_json::Value;
use sha2::{Digest, Sha256};

pub fn text(value: &Value) -> String {
    match value {
        Value::Null => "None".into(),
        Value::Bool(true) => "True".into(),
        Value::Bool(false) => "False".into(),
        Value::String(s) => s.clone(),
        _ => value.to_string(),
    }
}

pub fn truth(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|v| v != 0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

pub fn string_or_empty(value: &Value) -> String {
    if truth(value) {
        text(value)
    } else {
        String::new()
    }
}

pub fn array(value: &Value) -> &[Value] {
    value.as_array().map(Vec::as_slice).unwrap_or(&[])
}

pub fn number(value: &Value) -> Option<f64> {
    match value {
        Value::String(s) => s.parse().ok(),
        Value::Bool(b) => Some(u8::from(*b) as f64),
        _ => value.as_f64(),
    }
}

pub fn valid_box(value: &Value) -> Option<[f64; 4]> {
    let a = value.as_array()?;
    if a.len() != 4 {
        return None;
    }
    let b = [
        number(&a[0])?,
        number(&a[1])?,
        number(&a[2])?,
        number(&a[3])?,
    ];
    (b.iter().all(|v| v.is_finite()) && b[2] > b[0] && b[3] > b[1]).then_some(b)
}

pub fn python_json(value: &Value, ascii: bool) -> String {
    fn quote(s: &str, ascii: bool) -> String {
        let escaped = serde_json::to_string(s).expect("string JSON");
        if !ascii {
            return escaped;
        }
        let mut output = String::new();
        for c in escaped.chars() {
            if (c as u32) < 127 {
                output.push(c);
            } else {
                for unit in c.encode_utf16(&mut [0; 2]) {
                    output.push_str(&format!("\\u{unit:04x}"));
                }
            }
        }
        output
    }
    match value {
        Value::String(s) => quote(s, ascii),
        Value::Array(a) => format!(
            "[{}]",
            a.iter()
                .map(|v| python_json(v, ascii))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Value::Object(o) => {
            let mut keys: Vec<_> = o.keys().collect();
            keys.sort();
            format!(
                "{{{}}}",
                keys.iter()
                    .map(|k| format!("{}: {}", quote(k, ascii), python_json(&o[*k], ascii)))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        }
        Value::Number(n) if n.is_f64() => {
            // Python uses a sign and at least two exponent digits.
            let raw = n.to_string();
            if let Some((mantissa, exponent)) = raw.split_once('e') {
                let e: i32 = exponent.parse().expect("JSON exponent");
                format!("{mantissa}e{e:+03}")
            } else {
                raw
            }
        }
        _ => value.to_string(),
    }
}

pub fn digest(value: &Value, ascii: bool) -> String {
    format!("{:x}", Sha256::digest(python_json(value, ascii).as_bytes()))
}
