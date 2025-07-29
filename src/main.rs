use std::{
    collections::HashMap,
    error::Error,
    fmt,
    io::{BufRead, BufReader, Lines, Write},
    net::{TcpListener, TcpStream},
    result::Result,
    str::FromStr,
};

// default logging used by Nginx
// log_format combined '$remote_addr - $remote_user [$time_local] '
//                     '"$request" $status $body_bytes_sent '
//                     '"$http_referer" "$http_user_agent"';
fn write_request_log() {}

#[derive(Debug, Clone, Copy, PartialEq)]
enum HttpVersion {
    HTTP1_0,
    HTTP1_1,
    HTTP2_0,
}

impl fmt::Display for HttpVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let version = match self {
            HttpVersion::HTTP1_0 => "HTTP/1.0",
            HttpVersion::HTTP1_1 => "HTTP/1.1",
            HttpVersion::HTTP2_0 => "HTTP/2.0",
        };
        write!(f, "{}", version)
    }
}

impl FromStr for HttpVersion {
    type Err = &'static str;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "HTTP/1.0" => Ok(HttpVersion::HTTP1_0),
            "HTTP/1.1" => Ok(HttpVersion::HTTP1_1),
            "HTTP/2.0" => Ok(HttpVersion::HTTP2_0),
            _ => Err("Unsupported HTTP version"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum HttpMethod {
    Get,
}

impl fmt::Display for HttpMethod {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let method = match self {
            HttpMethod::Get => "GET",
        };
        write!(f, "{}", method)
    }
}

impl FromStr for HttpMethod {
    type Err = &'static str;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "GET" => Ok(HttpMethod::Get),
            _ => Err("Unsupported HTTP method"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum HttpStatus {
    Ok,
    MethodNotAllowed,
    HttpVersionNotSupported,
}

impl fmt::Display for HttpStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let status_text = match self {
            HttpStatus::Ok => "Ok",
            HttpStatus::MethodNotAllowed => "Method Not Allowed",
            HttpStatus::HttpVersionNotSupported => "Http Version Not Supported",
        };
        write!(f, "{}", status_text)
    }
}

impl HttpStatus {
    fn code(&self) -> u16 {
        match &self {
            HttpStatus::Ok => 200,
            HttpStatus::MethodNotAllowed => 405,
            HttpStatus::HttpVersionNotSupported => 505,
        }
    }
}

/// HTTP headers defined as a type alias to HashMap<String, String>
type HttpHeaders = HashMap<String, String>;

#[derive(Debug, Clone, PartialEq)]
struct HttpRequest {
    method: HttpMethod,
    path: String,
    version: HttpVersion,
    headers: Option<HttpHeaders>,
    body: Option<String>,
}

impl HttpRequest {
    fn new(
        method: HttpMethod,
        path: String,
        version: HttpVersion,
        headers: Option<HttpHeaders>,
        body: Option<String>,
    ) -> Result<Self, Box<dyn Error>> {
        Ok(HttpRequest {
            method,
            path,
            version,
            headers,
            body,
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
struct HttpResponse {
    version: HttpVersion,
    status: HttpStatus,
    headers: Option<HttpHeaders>,
    body: Option<String>,
}

impl HttpResponse {
    fn new(
        status: HttpStatus,
        headers: Option<HttpHeaders>,
        body: Option<String>,
    ) -> Result<Self, Box<dyn Error>> {
        Ok(HttpResponse {
            version: HttpVersion::HTTP1_1,
            status,
            headers,
            body,
        })
    }
}

impl fmt::Display for HttpResponse {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let body = self.body.as_deref().unwrap_or("");

        // status line
        writeln!(f, "{} {} {}", self.version, self.status.code(), self.status)?;

        if !body.is_empty() {
            writeln!(f, "Content-Length: {}", body.len())?;
            writeln!(f, "Content-Type: text/plain")?;
        }

        // headers
        if let Some(headers) = &self.headers {
            for (field_name, field_value) in headers.iter() {
                writeln!(f, "{}: {}", field_name, field_value)?;
            }
        }

        // empty line
        writeln!(f)?;

        // body
        if !body.is_empty() {
            writeln!(f, "{}", body)?;
        }

        Ok(())
    }
}

fn parse_response_headers(
    lines: &mut Lines<BufReader<&TcpStream>>,
) -> Result<HttpHeaders, Box<dyn Error>> {
    let mut headers: HashMap<String, String> = HashMap::new();
    let mut has_host = false;

    for line in lines {
        let line = line?;

        // end of headers
        if line.is_empty() {
            break;
        };

        let (field_name, field_value) = line
            .split_once(":")
            .map(|(f1, f2)| (f1.trim(), f2.trim()))
            .ok_or_else(|| format!("Malformed header: {}", line))?;

        // TODO: Currently the parsing of headers does not NOT conform to rfc9110.
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

fn send_response(mut stream: TcpStream, response: HttpResponse) -> Result<(), Box<dyn Error>> {
    write!(stream, "{}", response)?;

    stream.flush()?;

    Ok(())
}

fn handle_connection(stream: TcpStream) -> Result<(), Box<dyn Error>> {
    let mut lines = BufReader::new(&stream).lines();

    let Some(request_line) = lines.next().transpose()? else {
        return Err("Empty request".into());
    };

    let [method_str, target, version_str] = request_line
        .split_whitespace()
        .collect::<Vec<&str>>()
        .try_into()
        .map_err(|_| "Invalid request line format")?;
    let headers = parse_response_headers(&mut lines)?;

    let method = method_str.parse()?;
    let version = version_str.parse()?;

    if method != HttpMethod::Get {
        let response = HttpResponse::new(HttpStatus::MethodNotAllowed, None, None)?;
        send_response(stream, response)?;
        return Ok(());
    }

    if version != HttpVersion::HTTP1_1 {
        let response = HttpResponse::new(HttpStatus::HttpVersionNotSupported, None, None)?;
        send_response(stream, response)?;
        return Ok(());
    }

    let request = HttpRequest::new(method, target.to_string(), version, headers, None)?;
    println!("{:#?}", request);

    let response = HttpResponse::new(
        HttpStatus::Ok,
        None,
        Some(
            request
                .headers
                .iter()
                .map(|h| h.to_string())
                .collect::<Vec<_>>()
                .join("\n"),
        ),
    )?;

    send_response(stream, response)?;

    Ok(())
}

fn main() -> Result<(), Box<dyn Error>> {
    let listener = TcpListener::bind("127.0.0.1:9000")?;

    for stream in listener.incoming() {
        handle_connection(stream?)?;
    }

    Ok(())
}
