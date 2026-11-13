//! The little web server a phone downloads from: reading its requests and
//! writing its answers. The sockets live in the app; everything that decides
//! what to answer is here, where it can be tested.
//!
//! Every address starts with a one-time token, so nobody else on the network
//! can guess their way to the files.

use std::fmt::Write;

use crate::files::{extension, format_size};

/// The longest request head accepted. Phones send well under 2 KB.
pub const MAX_HEAD: usize = 8 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    Get,
    Head,
    Other,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    pub method: Method,
    pub path: String,
    /// The raw value of a `Range` header, if there was one.
    pub range: Option<String>,
}

/// Reads the head of a request, up to the blank line.
pub fn parse_request(head: &str) -> Option<Request> {
    let mut lines = head.split("\r\n");
    let mut first = lines.next()?.split(' ');
    let method = match first.next()? {
        "GET" => Method::Get,
        "HEAD" => Method::Head,
        _ => Method::Other,
    };
    let path = first.next()?.to_owned();
    if !first.next()?.starts_with("HTTP/1.") {
        return None;
    }
    let range = lines.find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.trim()
            .eq_ignore_ascii_case("range")
            .then(|| value.trim().to_owned())
    });
    Some(Request {
        method,
        path,
        range,
    })
}

/// Where a request goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Route {
    /// The page listing what is offered.
    Page,
    /// One of the files, by its place in the list.
    File(usize),
    NotFound,
}

/// Files live at `/{token}/{index}/{name}`. The name is only there so that a
/// browser that ignores the download header still saves under the right name.
pub fn route(path: &str, token: &str, files: usize) -> Route {
    let path = path.split(['?', '#']).next().unwrap_or_default();
    let mut parts = path.strip_prefix('/').unwrap_or(path).split('/');
    if parts.next() != Some(token) {
        return Route::NotFound;
    }
    match parts.next() {
        None | Some("") => Route::Page,
        Some(index) => match index.parse::<usize>() {
            Ok(index) if index < files => Route::File(index),
            _ => Route::NotFound,
        },
    }
}

/// Part of a file asked for, as the first and last byte, both included.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Span {
    Whole,
    Part {
        first: u64,
        last: u64,
    },
    /// Starts past the end, which gets 416.
    Unsatisfiable,
}

/// Reads a single range such as `bytes=0-1023`, `bytes=500-` or `bytes=-500`.
/// Several ranges at once are rare, and answering with the whole file is
/// allowed, so those get the whole file.
pub fn parse_range(value: Option<&str>, size: u64) -> Span {
    let Some(spec) = value.and_then(|value| value.trim().strip_prefix("bytes=")) else {
        return Span::Whole;
    };
    if spec.contains(',') {
        return Span::Whole;
    }
    let Some((start, end)) = spec.split_once('-') else {
        return Span::Whole;
    };
    let (start, end) = (start.trim(), end.trim());
    let span = match (start.parse::<u64>(), end.parse::<u64>()) {
        (Ok(first), Ok(last)) if first <= last => Some((first, last.min(size.saturating_sub(1)))),
        (Ok(first), Err(_)) if end.is_empty() => Some((first, size.saturating_sub(1))),
        (Err(_), Ok(suffix)) if start.is_empty() && suffix > 0 => {
            Some((size.saturating_sub(suffix), size.saturating_sub(1)))
        }
        _ => None,
    };
    match span {
        Some((first, _)) if first >= size => Span::Unsatisfiable,
        Some((first, last)) => Span::Part { first, last },
        None => Span::Whole,
    }
}

/// Encodes a name for a URL path or the `filename*` of a download header.
pub fn percent_encode(text: &str) -> String {
    let mut encoded = String::with_capacity(text.len() * 3);
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) {
            encoded.push(byte as char);
        } else {
            let _ = write!(encoded, "%{byte:02X}");
        }
    }
    encoded
}

/// Tells the browser to save the file under its own name. The plain name is a
/// fallback for old browsers, with anything unusual replaced.
pub fn content_disposition(name: &str) -> String {
    let fallback: String = name
        .chars()
        .map(|character| {
            if character.is_ascii_graphic() && character != '"' && character != '\\'
                || character == ' '
            {
                character
            } else {
                '_'
            }
        })
        .collect();
    format!(
        "attachment; filename=\"{fallback}\"; filename*=UTF-8''{}",
        percent_encode(name)
    )
}

pub fn content_type(name: &str) -> &'static str {
    match extension(name).as_str() {
        "jpg" | "jpeg" | "jpe" | "jfif" => "image/jpeg",
        "png" => "image/png",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "heic" => "image/heic",
        "heif" => "image/heif",
        "mp4" | "m4v" => "video/mp4",
        "mov" => "video/quicktime",
        "mp3" => "audio/mpeg",
        "m4a" => "audio/mp4",
        "pdf" => "application/pdf",
        "zip" => "application/zip",
        "txt" => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

fn escape_html(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&#39;"),
            _ => escaped.push(character),
        }
    }
    escaped
}

/// The page the phone opens: each file with a button that downloads it.
pub fn page(token: &str, files: &[(String, u64)]) -> String {
    let mut rows = String::new();
    for (index, (name, size)) in files.iter().enumerate() {
        let _ = write!(
            rows,
            "<li><div><b>{}</b><span>{}</span></div><a href=\"/{token}/{index}/{}\" download>İndir</a></li>",
            escape_html(name),
            format_size(*size),
            percent_encode(name),
        );
    }
    let title = if files.len() == 1 {
        "Dosya hazır"
    } else {
        "Dosyalar hazır"
    };
    format!(
        "<!doctype html><html lang=\"tr\"><head><meta charset=\"utf-8\">\
<meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">\
<title>Islora</title><style>\
body{{margin:0;font:16px/1.4 system-ui,sans-serif;background:#0b0b0d;color:#f2f2f4}}\
main{{max-width:520px;margin:0 auto;padding:32px 20px}}\
h1{{font-size:22px;margin:0 0 4px}}p{{margin:0 0 24px;color:#9a9aa2}}\
ul{{list-style:none;margin:0;padding:0}}\
li{{display:flex;align-items:center;gap:12px;padding:14px 0;border-top:1px solid #222}}\
li div{{flex:1;min-width:0}}b{{display:block;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}}\
span{{color:#9a9aa2;font-size:14px}}\
a{{background:#2fbf71;color:#04170c;font-weight:600;text-decoration:none;padding:10px 18px;border-radius:999px}}\
</style></head><body><main><h1>{title}</h1>\
<p>Islora bu bilgisayardan gönderiyor. Bağlantı yalnızca bu ağda ve kısa bir süre açık.</p>\
<ul>{rows}</ul></main></body></html>"
    )
}

/// The first lines of an answer, up to and including the blank line.
pub fn response_head(status: u16, headers: &[(&str, String)]) -> String {
    let reason = match status {
        200 => "OK",
        206 => "Partial Content",
        404 => "Not Found",
        405 => "Method Not Allowed",
        416 => "Range Not Satisfiable",
        _ => "Error",
    };
    let mut head = format!("HTTP/1.1 {status} {reason}\r\n");
    for (name, value) in headers {
        let _ = write!(head, "{name}: {value}\r\n");
    }
    head.push_str("Connection: close\r\nCache-Control: no-store\r\n\r\n");
    head
}

/// A token for the address, from random bytes: 20 letters and digits, about
/// 100 bits, which nobody guesses within the few minutes it lives.
pub fn token(random: &[u8; 13]) -> String {
    const ALPHABET: &[u8; 32] = b"abcdefghijklmnopqrstuvwxyz234567";
    let mut bits = 0_u32;
    let mut held = 0;
    let mut token = String::with_capacity(20);
    for &byte in random {
        bits = (bits << 8) | u32::from(byte);
        held += 8;
        while held >= 5 && token.len() < 20 {
            held -= 5;
            token.push(ALPHABET[((bits >> held) & 31) as usize] as char);
        }
    }
    token
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_head_is_read() {
        let head =
            "GET /abc/0/a.jpg HTTP/1.1\r\nHost: 192.168.1.5:5000\r\nrange: bytes=0-1\r\n\r\n";
        let request = parse_request(head).expect("valid");
        assert_eq!(request.method, Method::Get);
        assert_eq!(request.path, "/abc/0/a.jpg");
        assert_eq!(request.range.as_deref(), Some("bytes=0-1"));
        assert_eq!(
            parse_request("HEAD / HTTP/1.0\r\n\r\n").map(|r| r.method),
            Some(Method::Head)
        );
        assert_eq!(parse_request("garbage"), None);
        assert_eq!(parse_request("GET / SPDY\r\n"), None);
    }

    #[test]
    fn only_the_token_opens_anything() {
        assert_eq!(route("/tok", "tok", 2), Route::Page);
        assert_eq!(route("/tok/", "tok", 2), Route::Page);
        assert_eq!(route("/tok/1/b.png", "tok", 2), Route::File(1));
        assert_eq!(route("/tok/1", "tok", 2), Route::File(1));
        assert_eq!(route("/tok/2/c", "tok", 2), Route::NotFound);
        assert_eq!(route("/tok/x/c", "tok", 2), Route::NotFound);
        assert_eq!(route("/other/0/a", "tok", 2), Route::NotFound);
        assert_eq!(route("/", "tok", 2), Route::NotFound);
        assert_eq!(route("/favicon.ico", "tok", 2), Route::NotFound);
        assert_eq!(route("/tok?x=1", "tok", 2), Route::Page);
    }

    #[test]
    fn ranges_are_read_as_phones_send_them() {
        assert_eq!(parse_range(None, 100), Span::Whole);
        assert_eq!(
            parse_range(Some("bytes=0-1"), 100),
            Span::Part { first: 0, last: 1 }
        );
        assert_eq!(
            parse_range(Some("bytes=10-"), 100),
            Span::Part {
                first: 10,
                last: 99
            }
        );
        assert_eq!(
            parse_range(Some("bytes=-30"), 100),
            Span::Part {
                first: 70,
                last: 99
            }
        );
        assert_eq!(
            parse_range(Some("bytes=90-500"), 100),
            Span::Part {
                first: 90,
                last: 99
            }
        );
        assert_eq!(parse_range(Some("bytes=100-"), 100), Span::Unsatisfiable);
        assert_eq!(parse_range(Some("bytes=0-1,5-9"), 100), Span::Whole);
        assert_eq!(parse_range(Some("items=0-1"), 100), Span::Whole);
        assert_eq!(parse_range(Some("bytes=9-3"), 100), Span::Whole);
        assert_eq!(
            parse_range(Some("bytes=-500"), 100),
            Span::Part { first: 0, last: 99 }
        );
    }

    #[test]
    fn names_survive_the_trip() {
        assert_eq!(
            percent_encode("Tatil fotoğrafı.jpg"),
            "Tatil%20foto%C4%9Fraf%C4%B1.jpg"
        );
        assert_eq!(
            content_disposition("Rapor \"son\".pdf"),
            "attachment; filename=\"Rapor _son_.pdf\"; filename*=UTF-8''Rapor%20%22son%22.pdf"
        );
        assert_eq!(
            content_disposition("ağaç.png"),
            "attachment; filename=\"a_a_.png\"; filename*=UTF-8''a%C4%9Fa%C3%A7.png"
        );
        assert_eq!(content_type("IMG_1.HEIC"), "image/heic");
        assert_eq!(content_type("x.unknown"), "application/octet-stream");
    }

    #[test]
    fn the_page_escapes_names_and_links_each_file() {
        let page = page("tok", &[("<b>.txt".to_owned(), 2048)]);
        assert!(page.contains("&lt;b&gt;.txt"));
        assert!(!page.contains("<b>.txt"));
        assert!(page.contains("href=\"/tok/0/%3Cb%3E.txt\""));
        assert!(page.contains("2,0 KB"));
        assert!(page.contains("Dosya hazır"));
    }

    #[test]
    fn an_answer_ends_with_a_blank_line() {
        let head = response_head(206, &[("Content-Length", "2".to_owned())]);
        assert!(head.starts_with("HTTP/1.1 206 Partial Content\r\n"));
        assert!(head.contains("Content-Length: 2\r\n"));
        assert!(head.ends_with("\r\n\r\n"));
    }

    #[test]
    fn tokens_use_every_bit_and_only_safe_letters() {
        assert_eq!(token(&[0; 13]), "aaaaaaaaaaaaaaaaaaaa");
        assert_eq!(token(&[255; 13]), "77777777777777777777");
        let mixed = token(&[
            0x12, 0x34, 0x56, 0x78, 0x9a, 0xbc, 0xde, 0xf0, 1, 2, 3, 4, 5,
        ]);
        assert_eq!(mixed.len(), 20);
        assert!(
            mixed
                .chars()
                .all(|c| c.is_ascii_lowercase() || ('2'..='7').contains(&c))
        );
    }
}
