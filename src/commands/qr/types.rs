//! The kinds of QR code the interactive builder offers, with the fields each
//! needs and a pure `build` that turns the entered values into the string that
//! actually gets encoded. Keeping this as data (not UI) makes every payload
//! format independently testable.

use std::collections::HashMap;

pub type Values = HashMap<String, String>;

pub struct QrField {
    /// Key under which the entered value is stored.
    pub key: &'static str,
    /// Prompt label shown to the user.
    pub label: &'static str,
    /// Example value shown as a hint when the field is empty.
    pub placeholder: Option<&'static str>,
    /// When true the field may be left blank.
    pub optional: bool,
}

pub struct QrType {
    /// Name shown in the type picker, and the stable identity of the type.
    pub label: &'static str,
    /// One-line description shown next to the label.
    pub hint: &'static str,
    /// Ordered fields the user fills in.
    pub fields: &'static [QrField],
    /// Turn entered values into the string to encode.
    pub build: fn(&Values) -> String,
}

const fn field(key: &'static str, label: &'static str) -> QrField {
    QrField {
        key,
        label,
        placeholder: None,
        optional: false,
    }
}

const fn optional(key: &'static str, label: &'static str) -> QrField {
    QrField {
        key,
        label,
        placeholder: None,
        optional: true,
    }
}

const fn hinted(key: &'static str, label: &'static str, placeholder: &'static str) -> QrField {
    QrField {
        key,
        label,
        placeholder: Some(placeholder),
        optional: false,
    }
}

const fn hinted_optional(
    key: &'static str,
    label: &'static str,
    placeholder: &'static str,
) -> QrField {
    QrField {
        key,
        label,
        placeholder: Some(placeholder),
        optional: true,
    }
}

fn get(values: &Values, key: &str) -> String {
    values
        .get(key)
        .map(|v| v.trim().to_string())
        .unwrap_or_default()
}

/// Escape the characters that are significant inside a `WIFI:` payload.
fn escape_wifi(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        if matches!(ch, '\\' | ';' | ',' | ':' | '"') {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

/// Prefix a bare host with `https://` so URL codes open in a browser.
fn ensure_scheme(url: &str) -> String {
    if has_scheme(url) {
        url.to_string()
    } else {
        format!("https://{url}")
    }
}

/// Matches `scheme://`, where a scheme is a letter followed by letters, digits,
/// `+`, `.` or `-`.
fn has_scheme(url: &str) -> bool {
    let rest = match url.find("://") {
        Some(index) => &url[..index],
        None => return false,
    };
    let mut chars = rest.chars();
    match chars.next() {
        Some(first) if first.is_ascii_alphabetic() => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '.' | '-'))
}

/// Percent-encode a query parameter the way `URLSearchParams` does: spaces
/// become `+`, and everything outside the unreserved set is escaped.
fn encode_query_component(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'*' | b'-' | b'.' | b'_' => {
                out.push(*byte as char)
            }
            b' ' => out.push('+'),
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

pub static QR_TYPES: &[QrType] = &[
    QrType {
        label: "Text",
        hint: "Any plain text",
        fields: &[field("text", "Text")],
        build: |v| v.get("text").cloned().unwrap_or_default(),
    },
    QrType {
        label: "URL",
        hint: "Open a website",
        fields: &[hinted("url", "URL", "example.com")],
        build: |v| ensure_scheme(&get(v, "url")),
    },
    QrType {
        label: "Telephone",
        hint: "Dial a phone number",
        fields: &[hinted("number", "Phone number", "+15551234567")],
        build: |v| format!("tel:{}", get(v, "number")),
    },
    QrType {
        label: "SMS",
        hint: "Pre-filled text message",
        fields: &[
            hinted("number", "Phone number", "+15551234567"),
            optional("message", "Message"),
        ],
        build: |v| {
            let number = get(v, "number");
            let message = get(v, "message");
            if message.is_empty() {
                format!("SMSTO:{number}")
            } else {
                format!("SMSTO:{number}:{message}")
            }
        },
    },
    QrType {
        label: "Email",
        hint: "Pre-filled email",
        fields: &[
            hinted("to", "To", "name@example.com"),
            optional("subject", "Subject"),
            optional("body", "Body"),
        ],
        build: |v| {
            let to = get(v, "to");
            let mut params: Vec<String> = Vec::new();
            for key in ["subject", "body"] {
                let value = get(v, key);
                if !value.is_empty() {
                    params.push(format!("{key}={}", encode_query_component(&value)));
                }
            }
            if params.is_empty() {
                format!("mailto:{to}")
            } else {
                format!("mailto:{to}?{}", params.join("&"))
            }
        },
    },
    QrType {
        label: "Wi-Fi",
        hint: "Join a wireless network",
        fields: &[
            field("ssid", "Network name (SSID)"),
            optional("password", "Password"),
            hinted_optional("encryption", "Encryption (WPA/WEP/nopass)", "WPA"),
        ],
        build: |v| {
            let ssid = escape_wifi(&get(v, "ssid"));
            let password = escape_wifi(&get(v, "password"));
            let encryption = if password.is_empty() {
                "nopass".to_string()
            } else {
                let entered = get(v, "encryption").to_uppercase();
                if entered.is_empty() {
                    "WPA".to_string()
                } else {
                    entered
                }
            };
            let password_part = if encryption == "nopass" {
                String::new()
            } else {
                format!("P:{password};")
            };
            format!("WIFI:T:{encryption};S:{ssid};{password_part};")
        },
    },
    QrType {
        label: "Location",
        hint: "Geographic coordinates",
        fields: &[
            hinted("lat", "Latitude", "37.7749"),
            hinted("lng", "Longitude", "-122.4194"),
        ],
        build: |v| format!("geo:{},{}", get(v, "lat"), get(v, "lng")),
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    fn values(pairs: &[(&str, &str)]) -> Values {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    fn build(label: &str, pairs: &[(&str, &str)]) -> String {
        let qr_type = QR_TYPES
            .iter()
            .find(|t| t.label == label)
            .expect("unknown type");
        (qr_type.build)(&values(pairs))
    }

    #[test]
    fn text_passes_through_unchanged() {
        assert_eq!(build("Text", &[("text", "  hello  ")]), "  hello  ");
    }

    #[test]
    fn url_gets_a_scheme_when_bare() {
        assert_eq!(
            build("URL", &[("url", "example.com")]),
            "https://example.com"
        );
    }

    #[test]
    fn url_keeps_an_existing_scheme() {
        assert_eq!(
            build("URL", &[("url", "http://example.com")]),
            "http://example.com"
        );
        assert_eq!(
            build("URL", &[("url", "ftp://files.example")]),
            "ftp://files.example"
        );
    }

    #[test]
    fn a_colon_in_a_path_is_not_mistaken_for_a_scheme() {
        assert_eq!(
            build("URL", &[("url", "example.com/a://b")]),
            "https://example.com/a://b"
        );
    }

    #[test]
    fn telephone_uses_the_tel_scheme() {
        assert_eq!(
            build("Telephone", &[("number", "+15551234567")]),
            "tel:+15551234567"
        );
    }

    #[test]
    fn sms_appends_the_message_only_when_present() {
        assert_eq!(build("SMS", &[("number", "+1555")]), "SMSTO:+1555");
        assert_eq!(
            build("SMS", &[("number", "+1555"), ("message", "hi")]),
            "SMSTO:+1555:hi"
        );
    }

    #[test]
    fn email_without_extras_is_a_bare_mailto() {
        assert_eq!(build("Email", &[("to", "a@b.com")]), "mailto:a@b.com");
    }

    #[test]
    fn email_encodes_subject_and_body() {
        assert_eq!(
            build(
                "Email",
                &[("to", "a@b.com"), ("subject", "hi there"), ("body", "a&b")]
            ),
            "mailto:a@b.com?subject=hi+there&body=a%26b"
        );
    }

    #[test]
    fn email_omits_an_empty_field() {
        assert_eq!(
            build("Email", &[("to", "a@b.com"), ("body", "text")]),
            "mailto:a@b.com?body=text"
        );
    }

    #[test]
    fn wifi_defaults_to_wpa_when_a_password_is_given() {
        assert_eq!(
            build("Wi-Fi", &[("ssid", "Home"), ("password", "secret")]),
            "WIFI:T:WPA;S:Home;P:secret;;"
        );
    }

    #[test]
    fn wifi_without_a_password_is_an_open_network() {
        assert_eq!(
            build("Wi-Fi", &[("ssid", "Cafe")]),
            "WIFI:T:nopass;S:Cafe;;"
        );
    }

    #[test]
    fn wifi_uppercases_the_entered_encryption() {
        assert_eq!(
            build(
                "Wi-Fi",
                &[("ssid", "N"), ("password", "p"), ("encryption", "wep")]
            ),
            "WIFI:T:WEP;S:N;P:p;;"
        );
    }

    #[test]
    fn wifi_escapes_significant_characters() {
        assert_eq!(
            build("Wi-Fi", &[("ssid", "My;Net"), ("password", "a\\b")]),
            "WIFI:T:WPA;S:My\\;Net;P:a\\\\b;;"
        );
    }

    #[test]
    fn geo_joins_the_coordinates() {
        assert_eq!(
            build("Location", &[("lat", "37.7749"), ("lng", "-122.4194")]),
            "geo:37.7749,-122.4194"
        );
    }

    #[test]
    fn encodes_non_ascii_as_utf8_percent_escapes() {
        assert_eq!(encode_query_component("é"), "%C3%A9");
    }

    #[test]
    fn every_type_has_a_unique_label_and_at_least_one_field() {
        let mut labels = std::collections::HashSet::new();
        for qr_type in QR_TYPES {
            assert!(
                labels.insert(qr_type.label),
                "duplicate label: {}",
                qr_type.label
            );
            assert!(
                !qr_type.fields.is_empty(),
                "{} has no fields",
                qr_type.label
            );
        }
    }

    #[test]
    fn every_type_builds_something_encodable_from_blank_values() {
        // The builder must never hand the encoder an empty string, which the
        // QR encoder would reject.
        for qr_type in QR_TYPES {
            let built = (qr_type.build)(&values(&[]));
            if qr_type.label != "Text" {
                assert!(!built.is_empty(), "{} built nothing", qr_type.label);
            }
        }
    }
}
