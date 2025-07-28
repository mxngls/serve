use std::{
    error::Error,
    fmt,
    io::{BufRead, BufReader, Lines, Write},
    net::{TcpListener, TcpStream},
    result::Result,
};

// default logging used by Nginx
// log_format combined '$remote_addr - $remote_user [$time_local] '
//                     '"$request" $status $body_bytes_sent '
//                     '"$http_referer" "$http_user_agent"';
fn write_request_log() {}

static HTTP_PROTOCOL: &str = "HTTP/1.1";

enum HttpSupportedMethods {
    Get,
}

impl HttpSupportedMethods {
    fn as_str(&self) -> &'static str {
        match self {
            HttpSupportedMethods::Get => "GET",
        }
    }
}

#[derive(Debug)]
struct HttpHeader {
    field_name: String,
    field_value: String,
}

impl fmt::Display for HttpHeader {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}: {}", self.field_name, self.field_value)
    }
}

fn parse_response_headers(
    lines: Lines<BufReader<&TcpStream>>,
) -> Result<Vec<HttpHeader>, Box<dyn Error>> {
    let mut headers: Vec<HttpHeader> = Vec::new();
    let mut has_host = false;

    for line in lines {
        let line = line?;

        // end of headers
        if line.is_empty() {
            break;
        };

        let (field_name, field_value) = line
            .split_once(":")
            .ok_or_else(|| format!("Malformed header: {}", line))?;

        // TODO: Currently the parsing of headers does not NOT to rfc9110.
        // See: https://www.rfc-editor.org/rfc/rfc9110.html#name-field-order
        headers.push(HttpHeader {
            field_name: field_name.trim().to_owned(),
            field_value: field_value.trim().to_owned(),
        });

        // In HTTP/1.1 all headers **except** for the host header are optional
        if !has_host && field_name == "Host" && !field_value.is_empty() {
            has_host = true
        };
    }

    if !has_host {
        Err("Missing Host header".into())
    } else {
        Ok(headers)
    }
}

fn send_response(
    mut stream: TcpStream,
    response_status: &str,
    response_status_text: &str,
    response_body: String,
) -> Result<(), Box<dyn Error>> {
    let response_body_length = response_body.len();
    let response = [
        format!("{HTTP_PROTOCOL} {response_status} {response_status_text}"),
        format!("Content-Length: {response_body_length}",),
        format!("Content-Type: {}", "text/html"),
        "".to_string(),
        response_body,
    ]
    .join("\r\n");

    stream.write_all(response.as_bytes())?;
    stream.flush()?;

    Ok(())
}

fn handle_connection(stream: TcpStream) -> Result<(), Box<dyn Error>> {
    let mut lines = BufReader::new(&stream).lines();

    let Some(request_line) = lines.next().transpose()? else {
        return Err("Empty request".into());
    };

    let [request_method, _request_target, request_protocol] = request_line
        .split_whitespace()
        .collect::<Vec<&str>>()
        .try_into()
        .map_err(|_| "Invalid request line format")?;

    let mut response_status = "200";
    let mut response_status_text = "OK";
    let response_body = "".to_owned();

    if request_method != HttpSupportedMethods::Get.as_str() {
        response_status = "405";
        response_status_text = "Method Not Allowed";

        send_response(stream, response_status, response_status_text, response_body)?;

        return Ok(());
    } else if request_protocol != HTTP_PROTOCOL {
        response_status = "505";
        response_status_text = "HTTP version not supported";

        send_response(stream, response_status, response_status_text, response_body)?;

        return Ok(());
    }

    let headers = parse_response_headers(lines)?;

    println!("{}", request_line);

    for header in &headers {
        println!("{}: {}", header.field_name, header.field_value);
    }

    send_response(
        stream,
        response_status,
        response_status_text,
        headers
            .iter()
            .map(|h| h.to_string())
            .collect::<Vec<_>>()
            .join("<br>"),
    )?;

    Ok(())
}

fn main() -> Result<(), Box<dyn Error>> {
    let listener = TcpListener::bind("127.0.0.1:9000")?;

    for stream in listener.incoming() {
        handle_connection(stream?)?;
    }

    Ok(())
}
