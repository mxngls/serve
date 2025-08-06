use std::collections::HashMap;
use std::error::Error;
use std::fs::{File, OpenOptions};
use std::io::BufWriter;
use std::io::Write;
use std::sync::Mutex;

use jiff::Zoned;

use crate::{HttpRequest, HttpResponse};

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

pub trait Logger {
    fn new(log_path: String) -> Result<Self, Box<dyn Error>>
    where
        Self: std::marker::Sized;
    fn write_request_log(
        &self,
        request: &HttpRequest,
        response: &HttpResponse,
        addr: &str,
    ) -> Result<(), Box<dyn Error>>;
}

pub struct DefaultLogger {
    writer: Mutex<BufWriter<File>>,
    _format: LogFormat,
}

impl Logger for DefaultLogger {
    fn new(log_path: String) -> Result<Self, Box<dyn std::error::Error>> {
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
