use crate::types::Value;
use crate::Connection;

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
