use std::{
    collections::HashMap,
    error::Error,
    fmt,
    fs::{File, OpenOptions},
    io::{BufRead, BufReader, BufWriter, Lines, Write},
    net::{TcpListener, TcpStream, ToSocketAddrs},
    result::Result,
    str::FromStr,
    sync::Mutex,
};

use jiff::Zoned;

struct HttpFileServer {
    listener: TcpListener,
    logger: Logger,
}

impl HttpFileServer {
    fn new(
        host: &str,
        port: u16,
        log_path: Option<String>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let addr = format!("{}:{}", host, port);
        let socket_addr = addr
            .to_socket_addrs()?
            .next()
            .ok_or("Failed to resolve address")?;

        Ok(HttpFileServer {
            listener: TcpListener::bind(socket_addr)?,
            logger: Logger::new(log_path.unwrap_or("log.txt".to_string()))?,
        })
    }

    fn handle_connection(&self, stream: TcpStream) -> Result<(), Box<dyn Error>> {
        let mut lines = BufReader::new(&stream).lines();

        let Some(request_line) = lines.next().transpose()? else {
            return Err("Empty request".into());
        };

        let [method_str, target, version_str] = request_line
            .split_whitespace()
            .collect::<Vec<&str>>()
            .try_into()
            .map_err(|_| "Invalid request line format")?;
        let headers = self.parse_request_headers(&mut lines)?;

        let method = method_str.parse()?;
        let version = version_str.parse()?;

        if method != HttpMethod::Get {
            let response = HttpResponse::new(HttpStatus::MethodNotAllowed, None, None)?;
            self.send_response(&stream, &response)?;
            return Ok(());
        }

        if version != HttpVersion::HTTP1_1 {
            let response = HttpResponse::new(HttpStatus::HttpVersionNotSupported, None, None)?;
            self.send_response(&stream, &response)?;
            return Ok(());
        }

        let mut request =
            HttpRequest::new(method, target.to_string(), version, Some(headers), None)?;
        println!("{:#?}", request);

        let response_body = {
            let mut headers: Vec<String> = request
                .headers
                .as_ref()
                .unwrap_or(&Default::default())
                .iter()
                .map(|(key, value)| format!("{}: {}", key, value))
                .collect();

            headers.sort_by(|a, b| {
                let (k1, _) = a.split_once(":").unwrap_or_default();
                let (k2, _) = b.split_once(":").unwrap_or_default();
                k1.cmp(k2)
            });

            headers.insert(
                0,
                format!(
                    "Received request at {} with the following headers:\n",
                    Zoned::now().strftime("%d/%b/%Y:%H:%M:%S %z")
                ),
            );

            headers.join("\n")
        };
        let response = HttpResponse::new(HttpStatus::Ok, None, Some(response_body))?;

        self.send_response(&stream, &response)?;

        self.logger
            .write_request_log(&mut request, &response, &stream.peer_addr()?.to_string())?;

        Ok(())
    }

    fn run(&self) -> Result<(), Box<dyn std::error::Error>> {
        for stream in self.listener.incoming() {
            self.handle_connection(stream?)?;
        }
        Ok(())
    }

    fn parse_request_headers(
        &self,
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
            headers.insert(field_name.trim().to_owned(), field_value.trim().to_owned());

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
        &self,
        mut stream: &TcpStream,
        response: &HttpResponse,
    ) -> Result<(), Box<dyn Error>> {
        write!(stream, "{}", response)?;

        stream.flush()?;

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
            LogFormat::Combined => format!(
                "{remote_addr} - {remote_user} [{time_local}] \"{request_line}\" {status} {body_bytes_sent} \"{referer}\" \"{user_agent}\"",
            ),
        }
    }
}

struct Logger {
    writer: Mutex<BufWriter<File>>,
    _format: LogFormat,
}

impl Logger {
    fn new(log_path: String) -> Result<Self, Box<dyn std::error::Error>> {
        let log_file = OpenOptions::new()
            .append(true)
            .create(true)
            .open(log_path)?;

        Ok(Logger {
            writer: Mutex::new(BufWriter::new(log_file)),
            _format: LogFormat::Combined,
        })
    }

    // logs conform to the same format as Nginxs standard combined logs
    fn write_request_log(
        &self,
        request: &mut HttpRequest,
        response: &HttpResponse,
        remote_addr: &str,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let mut writer = self.writer.lock().unwrap();

        let headers = request.headers.take().unwrap_or_default();

        // TODO: Properly obtain remote user
        let remote_user = "\"-\"".to_string();
        let time_local = Zoned::now().strftime("%d/%b/%Y:%H:%M:%S %z").to_string();
        let request_line = format!(
            "{:?} {} {:?}",
            request.method, request.path, request.version
        );
        let status = response.status.code();
        let body_bytes_sent = response.body.as_ref().map(|b| b.len()).unwrap_or(0);
        let referer = headers.get("Referer").map_or("-", |h| h);
        let user_agent = headers.get("User-Agent").map_or("-", |h| h);

        // default logging used by Nginx (nginx)
        let log_line = LogFormat::Combined.format(
            remote_addr,
            &remote_user,
            &time_local,
            &request_line,
            status,
            body_bytes_sent,
            referer,
            user_agent,
        );

        write!(writer, "{}", log_line)?;

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

fn main() -> Result<(), Box<dyn Error>> {
    let server = HttpFileServer::new("localhost", 9000, Some("log.txt".to_string()))?;

    server.run()?;

    Ok(())
}
