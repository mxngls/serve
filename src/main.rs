use std::{
    error::Error,
    io::{BufRead, BufReader, Lines, Write},
    net::{TcpListener, TcpStream},
    result::Result,
};

use jiff::Zoned;

// default logging used by Nginx
// log_format combined '$remote_addr - $remote_user [$time_local] '
//                     '"$request" $status $body_bytes_sent '
//                     '"$http_referer" "$http_user_agent"';
fn write_request_log() {}

struct HttpHeader {
    field_name: String,
    field_value: String,
}

fn parse_response_headers(lines: Lines<BufReader<&TcpStream>>) -> Result<(), Box<dyn Error>> {
    let headers: Vec<HttpHeader> = Vec::new();

    for line in lines {
        let (field_name, field_value) = line?
            .split_once(":")
            .ok_or_else(|| format!("Malformed header: {}", line))?;

        headers.push(HttpHeader {
            field_name: field_name.trim(),
            field_value: field_value.trim(),
        });

        println!("{}", line);
    }
    Ok(())
}

fn send_response(
    mut stream: TcpStream,
    response_status: &str,
    response_status_text: &str,
    response_body: String,
) -> Result<(), Box<dyn Error>> {
    let response_body_length = response_body.len();
    let response = [
        format!("HTTP/1.1 {response_status} {response_status_text}"),
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

    let [request_method, _request_target, _request_protocol] = request_line
        .split_whitespace()
        .collect::<Vec<&str>>()
        .try_into()
        .map_err(|_| "Invalid request line format")?;

    let mut response_status = "200";
    let mut response_status_text = "OK";
    let response_body = format!("Currently it is {}", Zoned::now().time());

    if request_method != "GET" {
        response_status = "405";
        response_status_text = "Method Not Allowed";

        send_response(stream, response_status, response_status_text, response_body)?;

        return Ok(());
    }

    parse_response_headers(lines)?;

    send_response(stream, response_status, response_status_text, response_body)?;

    Ok(())
}

fn main() -> Result<(), Box<dyn Error>> {
    let listener = TcpListener::bind("127.0.0.1:9000")?;

    for stream in listener.incoming() {
        handle_connection(stream?)?;
    }

    Ok(())
}
