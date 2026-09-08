//! Child-process protocol for the named local Gateway fixture.

use std::io::{Read as _, Write as _};
use std::net::{TcpListener, TcpStream};

use super::{CONTROL_ADDRESS_ENV, EXPECTED_KEY_ENV, INSTANCE_LEASE_RUN_DIR_ENV};

/// Serves health, bearer, and shutdown requests inside the named child.
#[expect(
    clippy::expect_used,
    reason = "the isolated fixture process fails immediately with the failed invariant"
)]
pub(super) fn run() {
    let Ok(control_address) = std::env::var(CONTROL_ADDRESS_ENV) else {
        return;
    };
    let _instance_lease = std::env::var_os(INSTANCE_LEASE_RUN_DIR_ENV).map(|run_dir| {
        shared_sidecar::GatewayInstanceLease::try_acquire(std::path::Path::new(&run_dir))
            .expect("acquire named fixture process lease")
            .expect("the named fixture is the process lease owner")
    });
    let expected_key =
        std::env::var(EXPECTED_KEY_ENV).expect("the fixture child receives an expected key");
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind named fixture Gateway");
    let port = listener
        .local_addr()
        .expect("read named fixture address")
        .port();
    let mut control = TcpStream::connect(control_address).expect("connect fixture control");
    control
        .write_all(&port.to_be_bytes())
        .expect("announce named fixture readiness");

    for stream in listener.incoming() {
        let mut stream = stream.expect("accept named fixture request");
        let expected_key = expected_key.clone();
        let mut connection_control = control
            .try_clone()
            .expect("clone the fixture control connection");
        std::thread::spawn(move || {
            while let Some(shutdown) = answer_request(&mut stream, expected_key.as_bytes()) {
                if shutdown {
                    connection_control
                        .write_all(&[1])
                        .expect("report the accepted shutdown request");
                }
            }
        });
    }
}

fn answer_request(stream: &mut TcpStream, expected_key: &[u8]) -> Option<bool> {
    let mut buffer = [0_u8; 4096];
    let Ok(read) = stream.read(&mut buffer) else {
        return None;
    };
    if read == 0 {
        return None;
    }
    let request = &buffer[..read];
    let accepted = request.starts_with(b"GET /health ") || has_bearer(request, expected_key);
    let response = if accepted {
        "HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}"
    } else {
        "HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\n\r\n"
    };
    stream
        .write_all(response.as_bytes())
        .is_ok()
        .then(|| accepted && request.starts_with(b"POST /shutdown "))
}

fn has_bearer(request: &[u8], expected_key: &[u8]) -> bool {
    request.split(|byte| *byte == b'\n').any(|line| {
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        let Some(colon) = line.iter().position(|byte| *byte == b':') else {
            return false;
        };
        if !line[..colon].eq_ignore_ascii_case(b"authorization") {
            return false;
        }
        let value = line[colon + 1..].trim_ascii_start();
        let Some(space) = value.iter().position(|byte| *byte == b' ') else {
            return false;
        };
        value[..space].eq_ignore_ascii_case(b"bearer") && &value[space + 1..] == expected_key
    })
}

#[test]
fn bearer_credentials_are_byte_exact_and_case_sensitive() {
    let expected = b"CaseSensitive-Key";
    assert!(has_bearer(
        b"GET / HTTP/1.1\r\naUtHoRiZaTiOn: bEaReR CaseSensitive-Key\r\n\r\n",
        expected
    ));

    let listener = TcpListener::bind("127.0.0.1:0").expect("bind credential regression");
    let address = listener.local_addr().expect("read regression address");
    let client = std::thread::spawn(move || {
        let mut stream = TcpStream::connect(address).expect("connect credential regression");
        stream
            .write_all(
                b"GET /protected HTTP/1.1\r\nAuthorization: Bearer casesensitive-key\r\n\r\n",
            )
            .expect("send wrong-case credential");
        stream
            .shutdown(std::net::Shutdown::Write)
            .expect("finish fixture request");
        let mut response = String::new();
        stream
            .read_to_string(&mut response)
            .expect("read fixture response");
        response
    });
    let (mut server, _) = listener.accept().expect("accept credential regression");
    assert!(
        !answer_request(&mut server, expected).expect("the fixture answers the request"),
        "an unauthorized request cannot report shutdown"
    );
    drop(server);
    let response = client.join().expect("credential regression client joins");
    assert!(
        response.starts_with("HTTP/1.1 401 Unauthorized"),
        "the shared fixture rejects a bearer that differs only by case: {response}"
    );
}
