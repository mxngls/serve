#![allow(clippy::missing_errors_doc)]

use std::collections::HashMap;
use std::error::Error;
use std::fmt;
use std::fs;
use std::io::{BufRead, BufReader, ErrorKind, Lines, Write};
use std::net::{TcpListener, TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::result::Result;
use std::str::FromStr;

use crate::Logger;

pub struct HttpFileServer<T: Logger> {
    listener: TcpListener,
    root_dir: PathBuf,
    logger: T,
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

impl<T: Logger> HttpFileServer<T> {
    pub fn new(
        host: &str,
        port: u16,
        root_dir: PathBuf,
        logger: T,
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
        for stream in self.listener.incoming() {
            self.handle_connection(&stream?);
        }
        Ok(())
    }

    fn handle_connection(&self, stream: &TcpStream) {
        let peer_addr = stream
            .peer_addr()
            .map_or("unknown".to_string(), |addr| addr.to_string());

        let request = match HttpRequest::from_stream(stream) {
            Ok(Some(request)) => request,
            Ok(None) => return,
            Err(e) => {
                let status = e.into();
                return Self::send_response(stream, &HttpResponse::new(status, None, None));
            }
        };

        let response = self.process_request(&request);

        Self::send_response(stream, &response);

        let _ = self
            .logger
            .write_request_log(&request, &response, &peer_addr)
            .inspect_err(|e| eprintln!("Failed to write request log: {e}"));
    }

    fn process_request(&self, request: &HttpRequest) -> HttpResponse {
        if request.method != HttpMethod::Get {
            return HttpResponse::new(HttpStatus::MethodNotAllowed, None, None);
        }

        if request.version != HttpVersion::HTTP1_0 {
            return HttpResponse::new(HttpStatus::HttpVersionNotSupported, None, None);
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
                return HttpResponse::new(status, None, None);
            }
        };
        let Ok(response_body) = fs::read_to_string(&path) else {
            return HttpResponse::new(HttpStatus::InternalServerError, None, None);
        };

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

        HttpResponse::new(HttpStatus::Ok, Some(headers), Some(response_body))
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

    fn send_response(stream: &TcpStream, response: &HttpResponse) {
        let mut stream = stream;

        if write!(stream, "{response}").is_err() {
            let error_response = HttpResponse::new(HttpStatus::InternalServerError, None, None);
            let _ = write!(stream, "{error_response}");
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpVersion {
    HTTP1_0,
}

impl fmt::Display for HttpVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let version = match self {
            Self::HTTP1_0 => "HTTP/1.0",
        };
        write!(f, "{version}")
    }
}

impl FromStr for HttpVersion {
    type Err = ParseRequestError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "HTTP/1.0" => Ok(Self::HTTP1_0),
            _ => Err(ParseRequestError::UnsupportedHttpVersion),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpMethod {
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
    type Err = ParseRequestError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "GET" => Ok(Self::Get),
            _ => Err(ParseRequestError::InvalidMethod),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpStatus {
    BadRequest,
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
            Self::BadRequest => "Bad Request",
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

impl From<ParseRequestError> for HttpStatus {
    fn from(error: ParseRequestError) -> Self {
        match error {
            ParseRequestError::UnsupportedHttpVersion => Self::HttpVersionNotSupported,
            ParseRequestError::MalformedRequest(_)
            | ParseRequestError::InvalidMethod
            | ParseRequestError::InvalidUri => Self::BadRequest,
            ParseRequestError::IoError(_) => Self::InternalServerError,
        }
    }
}

impl HttpStatus {
    pub const fn code(self) -> u16 {
        match &self {
            Self::BadRequest => 400,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpRequest {
    pub method: HttpMethod,
    pub uri: String,
    pub version: HttpVersion,
    pub headers: Option<HttpHeaders>,
    pub body: Option<String>,
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

    pub fn from_stream(stream: &TcpStream) -> Result<Option<Self>, ParseRequestError> {
        let mut lines = BufReader::new(stream).lines();

        let Some(request_line) = lines.next().transpose()? else {
            return Ok(None);
        };

        let [method_str, target, version_str] = request_line
            .split_whitespace()
            .collect::<Vec<&str>>()
            .try_into()
            .map_err(|_| {
                ParseRequestError::MalformedRequest(format!(
                    "Malformed request line: {request_line}"
                ))
            })?;
        let headers = Self::parse_headers(&mut lines)?;

        let method = method_str.parse()?;
        let version = version_str.parse()?;

        let request = Self::new(method, target.to_string(), version, Some(headers), None);

        Ok(Some(request))
    }

    fn parse_headers(
        lines: &mut Lines<BufReader<&TcpStream>>,
    ) -> Result<HttpHeaders, ParseRequestError> {
        let mut headers = HttpHeaders::new();

        for line in lines {
            let line = line?;

            // end of headers
            if line.is_empty() {
                break;
            }

            let (field_name, field_value) = line
                .split_once(':')
                .map(|(f1, f2)| (f1.trim(), f2.trim()))
                .ok_or_else(|| {
                    ParseRequestError::MalformedRequest(format!("Malformed header: {line}"))
                })?;

            // TODO: Currently the parsing of headers does not NOT conform to rfc9110.
            // See: https://www.rfc-editor.org/rfc/rfc9110.html#name-field-order
            headers.insert(field_name.trim().to_owned(), field_value.trim().to_owned());
        }

        Ok(headers)
    }
}

#[derive(Debug)]
pub enum ParseRequestError {
    UnsupportedHttpVersion,
    MalformedRequest(String),
    InvalidMethod,
    InvalidUri,
    IoError(std::io::Error),
}

impl Error for ParseRequestError {}

impl fmt::Display for ParseRequestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedHttpVersion => f.write_str("Unsupported HTTP version"),
            Self::MalformedRequest(msg) => write!(f, "Malformed request: {msg}"),
            Self::InvalidMethod => f.write_str("Invalid method"),
            Self::InvalidUri => f.write_str("Invalid URI"),
            Self::IoError(err) => write!(f, "I/O error: {err}"),
        }
    }
}

impl From<std::io::Error> for ParseRequestError {
    fn from(error: std::io::Error) -> Self {
        Self::IoError(error)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpResponse {
    pub version: HttpVersion,
    pub status: HttpStatus,
    pub headers: Option<HttpHeaders>,
    pub body: Option<String>,
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
            version: HttpVersion::HTTP1_0,
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
