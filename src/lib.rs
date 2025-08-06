#![allow(clippy::missing_errors_doc)]

use std::collections::HashMap;
use std::error::Error;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, BufWriter, ErrorKind, Lines, Write};
use std::net::{TcpListener, TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::result::Result;
use std::str::FromStr;
use std::sync::Mutex;

use jiff::Zoned;

pub struct HttpFileServer {
    listener: TcpListener,
    root_dir: PathBuf,
    logger: Logger,
}

/// taken from <https://developer.mozilla.org/en-US/docs/Web/HTTP/Guides/MIME_types/Common_types/>
/// NOTE: This list is __obviously__ not exhaustive yet but should for now suffice for our needs for
/// now
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mime {
    GZip,
    Json,
    JsonLd,
    Binary,
    Xml,
    Zip,
    Gif,
    Jpeg,
    Png,
    Svg,
    ICalendar,
    Css,
    Csv,
    Html,
    JavaScriptModule,
    JavaScript,
    Markdown,
    PlainText,
}

impl Mime {
    /// Returns possible file extensions for this MIME type
    const fn as_str(&self) -> &'static str {
        match self {
            Self::GZip => "application/gzip",
            Self::Json => "application/json",
            Self::JsonLd => "application/ld+json",
            Self::Binary => "application/octet-stream",
            Self::Xml => "application/xml",
            Self::Zip => "application/zip",
            Self::Gif => "image/gif",
            Self::Jpeg => "image/jpeg",
            Self::Png => "image/png",
            Self::Svg => "image/svg+xml",
            Self::ICalendar => "text/calendar",
            Self::Css => "text/css",
            Self::Csv => "text/csv",
            Self::Html => "text/html",
            Self::JavaScriptModule | Self::JavaScript => "text/javascript",
            Self::Markdown => "text/markdown",
            Self::PlainText => "text/plain",
        }
    }
}

impl FromStr for Mime {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "gz" => Ok(Self::GZip),
            "json" => Ok(Self::Json),
            "jsonld" => Ok(Self::JsonLd),
            "bin" => Ok(Self::Binary),
            "xml" => Ok(Self::Xml),
            "zip" => Ok(Self::Zip),
            "gif" => Ok(Self::Gif),
            "jpeg" | "jpg" => Ok(Self::Jpeg),
            "png" => Ok(Self::Png),
            "svg" => Ok(Self::Svg),
            "ics" => Ok(Self::ICalendar),
            "css" => Ok(Self::Css),
            "csv" => Ok(Self::Csv),
            // technically .htm would be valid here as well but in
            // the context of where and how we intent to use the
            // server we will omit this here on purpose
            "html" => Ok(Self::Html),
            "mjs" => Ok(Self::JavaScriptModule),
            "js" => Ok(Self::JavaScript),
            "md" => Ok(Self::Markdown),
            "txt" => Ok(Self::PlainText),
            _ => Err("Unkown file extension".into()),
        }
    }
}

impl HttpFileServer {
    pub fn new(
        host: &str,
        port: u16,
        root_dir: PathBuf,
        logger: Logger,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let addr = format!("{host}:{port}");
        let socket_addr = addr
            .to_socket_addrs()?
            .next()
            .ok_or("Failed to resolve address")?;

        Ok(Self {
            listener: TcpListener::bind(socket_addr)?,
            root_dir,
            logger,
        })
    }

    pub fn run(&self) -> Result<(), Box<dyn std::error::Error>> {
        for stream in self.listener.incoming() {}
        Ok(())
    }

    fn handle_connection(&self, stream: &TcpStream) -> Result<(), Box<dyn Error>> {
        let request = HttpRequest::from_stream(stream)?;

        if request.method != HttpMethod::Get {
            return Self::send_response(
                stream,
                &HttpResponse::new(HttpStatus::MethodNotAllowed, None, None),
            );
        }

        if request.version != HttpVersion::HTTP1_1 {
            return Self::send_response(
                stream,
                &HttpResponse::new(HttpStatus::HttpVersionNotSupported, None, None),
            );
        }

        // match the request URI to actual file paths
        let path = match Self::resolve_uri(request.uri.as_str(), &self.root_dir) {
            Ok(path) => path,
            Err(err) => {
                let status = match err.kind() {
                    ErrorKind::PermissionDenied => HttpStatus::Forbidden,
                    ErrorKind::NotFound => HttpStatus::NotFound,
                    _ => HttpStatus::InternalServerError,
                };
                return Self::send_response(stream, &HttpResponse::new(status, None, None));
            }
        };
        let response_body = fs::read_to_string(&path)?;

        // handle the case where ther might be no extension or invalid UTF-8
        let mime_type_str = path
            .extension()
            .and_then(|ext| ext.to_str())
            .and_then(|ext_str| ext_str.parse().ok())
            .map_or(Mime::Binary, |mime| mime)
            .as_str()
            .to_string();

        let mut headers = HttpHeaders::new();

        headers.insert("Content-Type".to_string(), mime_type_str);

        let response = HttpResponse::new(HttpStatus::Ok, Some(headers), Some(response_body));

        Self::send_response(stream, &response)?;
        self.logger
            .write_request_log(&request, &response, &stream.peer_addr()?.to_string())?;

        Ok(())
    }

    fn resolve_uri(path: &str, root: &Path) -> std::io::Result<PathBuf> {
        let path = path.trim_start_matches('/').trim_end_matches('/');
        let index = "index.html";

        if path.is_empty() {
            return Ok(root.join(index));
        }

        let full_path = root.join(path);

        let canonical_full = full_path.canonicalize()?;
        let canonical_root = root.canonicalize()?;

        if !canonical_full.starts_with(&canonical_root) {
            return Err(ErrorKind::PermissionDenied.into());
        }

        if canonical_full.is_file() {
            return Ok(canonical_full);
        }

        if !canonical_full.is_dir() {
            return Err(ErrorKind::NotFound.into());
        }

        let index_path = canonical_full.join(index);
        if index_path.is_file() {
            Ok(index_path)
        } else {
            Err(ErrorKind::NotFound.into())
        }
    }

    fn send_response(
        mut stream: &TcpStream,
        response: &HttpResponse,
    ) -> Result<(), Box<dyn Error>> {
        stream.write_all(response.to_string().as_bytes())?;

        Ok(())
    }
}

enum LogFormat {
    Combined,
}

#[allow(clippy::too_many_arguments)]
impl LogFormat {
    fn format(
        &self,
        remote_addr: &str,
        remote_user: &str,
        time_local: &str,
        request_line: &str,
        status: u16,
        body_bytes_sent: usize,
        referer: &str,
        user_agent: &str,
    ) -> String {
        match self {
            Self::Combined => format!(
                "{remote_addr} - {remote_user} [{time_local}] \"{request_line}\" {status} {body_bytes_sent} \"{referer}\" \"{user_agent}\"",
            ),
        }
    }
}

pub struct Logger {
    writer: Mutex<BufWriter<File>>,
    _format: LogFormat,
}

impl Logger {
    pub fn new(log_path: String) -> Result<Self, Box<dyn std::error::Error>> {
        let log_file = OpenOptions::new()
            .append(true)
            .create(true)
            .open(log_path)?;

        Ok(Self {
            writer: Mutex::new(BufWriter::new(log_file)),
            _format: LogFormat::Combined,
        })
    }

    // logs conform to the same format as Nginxs standard combined logs
    fn write_request_log(
        &self,
        request: &HttpRequest,
        response: &HttpResponse,
        remote_addr: &str,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let default = &HashMap::default();
        let headers = request.headers.as_ref().unwrap_or(default);

        // TODO: Properly obtain remote user
        let remote_user = "\"-\"".to_string();
        let time_local = Zoned::now().strftime("%d/%b/%Y:%H:%M:%S %z").to_string();
        let request_line = format!("{:?} {} {:?}", request.method, request.uri, request.version);
        let status = response.status.code();
        let body_bytes_sent = response.body.as_ref().map_or(0, String::len);
        let referer = headers.get("Referer").map_or("-", |h| h);
        let user_agent = headers.get("User-Agent").map_or("-", |h| h);

        let log_line = LogFormat::Combined.format(
            remote_addr,
            &remote_user,
            &time_local,
            &request_line,
            status,
            body_bytes_sent,
            referer,
            user_agent,
        ) + "\n";

        self.writer.lock().unwrap().write_all(log_line.as_bytes())?;

        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum HttpVersion {
    HTTP1_0,
    HTTP1_1,
    HTTP2_0,
}

impl fmt::Display for HttpVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let version = match self {
            Self::HTTP1_0 => "HTTP/1.0",
            Self::HTTP1_1 => "HTTP/1.1",
            Self::HTTP2_0 => "HTTP/2.0",
        };
        write!(f, "{version}")
    }
}

impl FromStr for HttpVersion {
    type Err = &'static str;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "HTTP/1.0" => Ok(Self::HTTP1_0),
            "HTTP/1.1" => Ok(Self::HTTP1_1),
            "HTTP/2.0" => Ok(Self::HTTP2_0),
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
            Self::Get => "GET",
        };
        write!(f, "{method}")
    }
}

impl FromStr for HttpMethod {
    type Err = &'static str;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "GET" => Ok(Self::Get),
            _ => Err("Unsupported HTTP method"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum HttpStatus {
    Forbidden,
    HttpVersionNotSupported,
    InternalServerError,
    MethodNotAllowed,
    NotFound,
    Ok,
}

impl fmt::Display for HttpStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let status_text = match self {
            Self::Forbidden => "Forbidden",
            Self::HttpVersionNotSupported => "Http Version Not Supported",
            Self::InternalServerError => "Interal Server Errror",
            Self::MethodNotAllowed => "Method Not Allowed",
            Self::NotFound => "Not Found",
            Self::Ok => "Ok",
        };
        write!(f, "{status_text}")
    }
}

impl HttpStatus {
    const fn code(self) -> u16 {
        match &self {
            Self::Forbidden => 403,
            Self::HttpVersionNotSupported => 505,
            Self::InternalServerError => 500,
            Self::MethodNotAllowed => 405,
            Self::NotFound => 404,
            Self::Ok => 200,
        }
    }
}

/// HTTP headers defined as a type alias to `HashMap<String, String>`
type HttpHeaders = HashMap<String, String>;

#[derive(Debug, Clone, PartialEq)]
pub struct HttpRequest {
    method: HttpMethod,
    uri: String,
    version: HttpVersion,
    headers: Option<HttpHeaders>,
    body: Option<String>,
}

impl HttpRequest {
    const fn new(
        method: HttpMethod,
        path: String,
        version: HttpVersion,
        headers: Option<HttpHeaders>,
        body: Option<String>,
    ) -> Self {
        Self {
            method,
            uri: path,
            version,
            headers,
            body,
        }
    }

    pub fn from_stream(stream: &TcpStream) -> Result<Self, Box<dyn Error>> {
        let mut lines = BufReader::new(stream).lines();

        let Some(request_line) = lines.next().transpose()? else {
            return Err("Empty request".into());
        };

        let [method_str, target, version_str] = request_line
            .split_whitespace()
            .collect::<Vec<&str>>()
            .try_into()
            .map_err(|_| "Invalid request line format")?;
        let headers = Self::parse_request_headers(&mut lines)?;

        let method = method_str.parse()?;
        let version = version_str.parse()?;

        let request = Self::new(method, target.to_string(), version, Some(headers), None);

        Ok(request)
    }

    fn parse_request_headers(
        lines: &mut Lines<BufReader<&TcpStream>>,
    ) -> Result<HttpHeaders, Box<dyn Error>> {
        let mut headers = HttpHeaders::new();
        let mut has_host = false;

        for line in lines {
            let line = line?;

            // end of headers
            if line.is_empty() {
                break;
            }

            let (field_name, field_value) = line
                .split_once(':')
                .map(|(f1, f2)| (f1.trim(), f2.trim()))
                .ok_or_else(|| format!("Malformed header: {line}"))?;

            // TODO: Currently the parsing of headers does not NOT conform to rfc9110.
            // See: https://www.rfc-editor.org/rfc/rfc9110.html#name-field-order
            headers.insert(field_name.trim().to_owned(), field_value.trim().to_owned());

            // In HTTP/1.1 all headers **except** for the host header are optional
            if !has_host && field_name == "Host" && !field_value.is_empty() {
                has_host = true;
            }
        }

        if has_host {
            Ok(headers)
        } else {
            Err("Missing Host header".into())
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct HttpResponse {
    version: HttpVersion,
    status: HttpStatus,
    headers: Option<HttpHeaders>,
    body: Option<String>,
}

impl HttpResponse {
    fn new(status: HttpStatus, headers: Option<HttpHeaders>, body: Option<String>) -> Self {
        let mut headers = headers.unwrap_or_default();

        // add default headers
        if let Some(ref body) = body {
            if !headers.contains_key("Content-Length") {
                let body_len = body.len();
                headers.insert("Content-Length".to_string(), body_len.to_string());
            }

            if !headers.contains_key("Content-Type") {
                headers.insert("Content-Type".to_string(), "text/plain".to_string());
            }
        }

        Self {
            version: HttpVersion::HTTP1_1,
            status,
            headers: Some(headers),
            body,
        }
    }
}

impl fmt::Display for HttpResponse {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let body = self.body.as_deref().unwrap_or("");

        // status line
        writeln!(f, "{} {} {}", self.version, self.status.code(), self.status)?;

        // headers
        if let Some(headers) = &self.headers {
            for (field_name, field_value) in headers {
                writeln!(f, "{field_name}: {field_value}")?;
            }
        }

        // empty line
        writeln!(f)?;

        // body
        if !body.is_empty() {
            writeln!(f, "{body}")?;
        }

        Ok(())
    }
}
