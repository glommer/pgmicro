use crate::ext::register_scalar_function;
use crate::types::Value;
use crate::Connection;
use turso_ext::{scalar, ExtensionApi, Value as ExtValue};

/// Register PostgreSQL-compatible scalar functions.
///
/// These are thin wrappers that map common PG function names to their Turso
/// equivalents, so that `DEFAULT now()`, `SELECT clock_timestamp()`, etc. work
/// without relying solely on translator-level rewriting.
pub fn register_pg_functions(ext_api: &mut ExtensionApi) {
    unsafe {
        register_scalar_function(ext_api.ctx, c"now".as_ptr(), pg_now);
        register_scalar_function(ext_api.ctx, c"clock_timestamp".as_ptr(), pg_now);
        register_scalar_function(ext_api.ctx, c"transaction_timestamp".as_ptr(), pg_now);
        register_scalar_function(ext_api.ctx, c"statement_timestamp".as_ptr(), pg_now);
    }
}

/// Returns the current timestamp as `YYYY-MM-DD HH:MM:SS.mmm`.
///
/// This is the Turso equivalent of PostgreSQL's `now()`, `clock_timestamp()`,
/// `transaction_timestamp()`, and `statement_timestamp()`. All four are mapped
/// to the same implementation since Turso does not distinguish between
/// transaction-time and wall-clock time.
#[scalar(name = "now")]
fn pg_now(_args: &[ExtValue]) -> ExtValue {
    let now = chrono::Utc::now();
    let formatted = now.format("%Y-%m-%d %H:%M:%S%.3f").to_string();
    ExtValue::from_text(formatted)
}

pub fn exec_pg_get_user_by_id(_oid: i64) -> Value {
    Value::build_text("turso")
}

pub fn exec_pg_is_visible(_oid: i64) -> Value {
    Value::from_i64(1)
}

pub fn exec_pg_encoding_to_char(encoding: i64) -> Value {
    let name = match encoding {
        6 => "UTF8",
        0 => "SQL_ASCII",
        _ => "UTF8",
    };
    Value::build_text(name)
}

pub fn exec_pg_get_constraintdef(conn: &Connection, oid: i64) -> Value {
    match crate::pg_catalog::pg_get_constraintdef(conn, oid) {
        Some(s) => Value::build_text(s),
        None => Value::Null,
    }
}

pub fn exec_pg_get_indexdef(conn: &Connection, oid: i64) -> Value {
    match crate::pg_catalog::pg_get_indexdef(conn, oid) {
        Some(s) => Value::build_text(s),
        None => Value::Null,
    }
}

pub fn exec_pg_format_type(type_oid: i64, typemod: i64) -> Value {
    let type_name = match type_oid {
        16 => "boolean".to_string(),
        17 => "bytea".to_string(),
        18 => "\"char\"".to_string(),
        19 => "name".to_string(),
        20 => "bigint".to_string(),
        21 => "smallint".to_string(),
        23 => "integer".to_string(),
        25 => "text".to_string(),
        26 => "oid".to_string(),
        114 => "json".to_string(),
        700 => "real".to_string(),
        701 => "double precision".to_string(),
        1000 => "boolean[]".to_string(),
        1007 => "integer[]".to_string(),
        1009 => "text[]".to_string(),
        1022 => "double precision[]".to_string(),
        1042 => {
            if typemod > 4 {
                format!("character({})", typemod - 4)
            } else {
                "character".to_string()
            }
        }
        1043 => {
            if typemod > 4 {
                format!("character varying({})", typemod - 4)
            } else {
                "character varying".to_string()
            }
        }
        1082 => "date".to_string(),
        1083 => "time without time zone".to_string(),
        1114 => "timestamp without time zone".to_string(),
        1184 => "timestamp with time zone".to_string(),
        1186 => "interval".to_string(),
        1700 => {
            if typemod > 4 {
                let precision = ((typemod - 4) >> 16) & 0xffff;
                let scale = (typemod - 4) & 0xffff;
                format!("numeric({precision},{scale})")
            } else {
                "numeric".to_string()
            }
        }
        2205 => "regclass".to_string(),
        2206 => "regtype".to_string(),
        2278 => "void".to_string(),
        2950 => "uuid".to_string(),
        3802 => "jsonb".to_string(),
        _ => "unknown".to_string(),
    };
    Value::build_text(type_name)
}

pub fn exec_lpad(input: &Value, length: usize, fill: &str) -> Value {
    let s = match input {
        Value::Text(t) => t.to_string(),
        Value::Null => return Value::Null,
        v => v.to_string(),
    };
    let char_count = s.chars().count();
    if char_count >= length {
        Value::build_text(s.chars().take(length).collect::<String>())
    } else {
        let fill_chars: Vec<char> = fill.chars().collect();
        if fill_chars.is_empty() {
            Value::build_text(s)
        } else {
            let pad: String = fill_chars
                .iter()
                .cycle()
                .take(length - char_count)
                .collect();
            Value::build_text(format!("{pad}{s}"))
        }
    }
}

fn gcd_inner(mut a: i64, mut b: i64) -> i64 {
    while b != 0 {
        let t = b;
        b = a % b;
        a = t;
    }
    a.wrapping_abs()
}

/// Greatest common divisor.
pub fn exec_gcd(a: i64, b: i64) -> Value {
    // PG raises ERROR on overflow (gcd(INT_MIN, 0)), we match that
    if (a == i64::MIN && b == 0) || (b == i64::MIN && a == 0) {
        return Value::build_text("ERROR: integer out of range");
    }
    if a == i64::MIN && b == i64::MIN {
        return Value::build_text("ERROR: integer out of range");
    }
    Value::from_i64(gcd_inner(a, b))
}

/// Least common multiple.
pub fn exec_lcm(a: i64, b: i64) -> Value {
    if a == 0 || b == 0 {
        return Value::from_i64(0);
    }
    let g = gcd_inner(a, b);
    match (a / g).checked_mul(b.wrapping_abs()) {
        Some(v) => Value::from_i64(v.wrapping_abs()),
        None => Value::build_text("ERROR: integer out of range"),
    }
}

/// Repeat a string n times.
pub fn exec_repeat(input: &Value, count: i64) -> Value {
    let s = match input {
        Value::Text(t) => t.as_str(),
        Value::Null => return Value::Null,
        _ => return Value::Null,
    };
    if count <= 0 {
        return Value::build_text(String::new());
    }
    Value::build_text(s.repeat(count as usize))
}

/// Simplified to_char: formats a number with the given format pattern.
/// Supports basic PG numeric format patterns (9, 0, S, MI, FM, D, G, PR, TH, L).
pub fn exec_to_char(value: &Value, format: &str) -> Value {
    let num = match value {
        Value::Null => return Value::Null,
        Value::Numeric(_) => value.as_float(),
        Value::Text(t) => match t.as_str().parse::<f64>() {
            Ok(f) => f,
            Err(_) => return Value::Null,
        },
        _ => return Value::Null,
    };

    let result = pg_to_char_numeric(num, format);
    Value::build_text(result)
}

/// pg_input_is_valid(text, type) → boolean
/// Returns true if the text is valid input for the given type.
pub fn exec_pg_input_is_valid(input: &Value, type_name: &str) -> Value {
    let s = match input {
        Value::Text(t) => t.as_str().to_string(),
        Value::Null => return Value::Null,
        v => v.to_string(),
    };
    let valid = crate::pg_catalog::validate_pg_input(&s, type_name).is_none();
    Value::from_i64(if valid { 1 } else { 0 })
}

/// Format a number using PG's to_char numeric format patterns.
fn pg_to_char_numeric(num: f64, format: &str) -> String {
    let is_negative = num < 0.0;
    let abs_num = num.abs();

    // Parse format string for flags
    let upper_fmt = format.to_uppercase();
    let fm = upper_fmt.contains("FM"); // fill mode (suppress padding)
    let has_pr = upper_fmt.contains("PR"); // angle brackets for negative
    let has_s = upper_fmt.contains('S'); // sign
    let has_mi = upper_fmt.starts_with("MI") || upper_fmt.ends_with("MI");

    // Count digit positions
    let mut integer_digits = 0;
    let mut decimal_digits = 0;
    let mut leading_zeros = 0;
    let mut seen_dot = false;

    for ch in upper_fmt.chars() {
        match ch {
            '9' => {
                if seen_dot {
                    decimal_digits += 1;
                } else {
                    integer_digits += 1;
                }
            }
            '0' => {
                if seen_dot {
                    decimal_digits += 1;
                } else {
                    integer_digits += 1;
                    leading_zeros += 1;
                }
            }
            'D' | '.' => seen_dot = true,
            _ => {}
        }
    }

    if integer_digits == 0 && decimal_digits == 0 {
        return format!("{num}");
    }

    // Format the number
    let formatted = if decimal_digits > 0 {
        let prec = decimal_digits;
        format!("{abs_num:.prec$}")
    } else {
        let int_val = abs_num as i64;
        format!("{int_val}")
    };

    // Split into integer and decimal parts
    let parts: Vec<&str> = formatted.split('.').collect();
    let int_part = parts[0];
    let dec_part = if parts.len() > 1 { parts[1] } else { "" };

    // Pad integer part
    let padded_int = if !fm {
        let width = integer_digits.max(int_part.len());
        if leading_zeros > 0 {
            format!("{int_part:0>width$}")
        } else {
            format!("{int_part:>width$}")
        }
    } else {
        int_part.to_string()
    };

    // Build result
    let mut result = if decimal_digits > 0 {
        format!("{padded_int}.{dec_part}")
    } else {
        padded_int
    };

    // Add sign
    if has_pr {
        result = if is_negative {
            format!("<{result}>")
        } else {
            format!(" {result} ")
        };
    } else if has_s {
        let sign_pos = upper_fmt.find('S').unwrap_or(0);
        let sign = if is_negative { "-" } else { "+" };
        if sign_pos == 0 {
            result = format!("{sign}{result}");
        } else {
            result = format!("{result}{sign}");
        }
    } else if has_mi {
        if is_negative {
            result = format!("{result}-");
        } else {
            result = format!("{result} ");
        }
    } else if is_negative {
        result = format!("-{result}");
    } else {
        result = format!(" {result}");
    }

    result
}

pub fn exec_rpad(input: &Value, length: usize, fill: &str) -> Value {
    let s = match input {
        Value::Text(t) => t.to_string(),
        Value::Null => return Value::Null,
        v => v.to_string(),
    };
    let char_count = s.chars().count();
    if char_count >= length {
        Value::build_text(s.chars().take(length).collect::<String>())
    } else {
        let fill_chars: Vec<char> = fill.chars().collect();
        if fill_chars.is_empty() {
            Value::build_text(s)
        } else {
            let pad: String = fill_chars
                .iter()
                .cycle()
                .take(length - char_count)
                .collect();
            Value::build_text(format!("{s}{pad}"))
        }
    }
}
