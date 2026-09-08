//! Crate-private local Gateway process for capability tests.

use std::io::{Read, Write as _};
use std::net::{TcpListener, TcpStream};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use shared_sidecar::{ConnectionFile, ValidatedConnection};

#[cfg(windows)]
const GATEWAY_EXE_NAME: &str = "promptforge-gateway.exe";
#[cfg(not(windows))]
const GATEWAY_EXE_NAME: &str = "promptforge-gateway";

const CONTROL_ADDRESS_ENV: &str = "PROMPTFORGE_TEST_GATEWAY_CONTROL_ADDRESS";
const EXPECTED_KEY_ENV: &str = "PROMPTFORGE_TEST_GATEWAY_EXPECTED_KEY";

pub(crate) struct ValidatedGateway {
    child: Child,
    port: u16,
    control: TcpStream,
    _directory: tempfile::TempDir,
}

impl ValidatedGateway {
    pub(crate) fn spawn(expected_key: &str) -> Self {
        let control = TcpListener::bind("127.0.0.1:0").expect("bind fixture control");
        control
            .set_nonblocking(true)
            .expect("make fixture control nonblocking");
        let directory = tempfile::TempDir::new().expect("create fixture executable directory");
        let executable = directory.path().join(GATEWAY_EXE_NAME);
        std::fs::copy(
            std::env::current_exe().expect("locate test executable"),
            &executable,
        )
        .expect("copy test executable under the Gateway image name");
        let mut child = Command::new(&executable)
            .args([
                "--exact",
                "test_gateway::validated_gateway_fixture_process",
                "--ignored",
            ])
            .env(
                CONTROL_ADDRESS_ENV,
                control
                    .local_addr()
                    .expect("read fixture control address")
                    .to_string(),
            )
            .env(EXPECTED_KEY_ENV, expected_key)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("start the named Gateway fixture process");
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut stream = loop {
            match control.accept() {
                Ok((stream, _)) => break stream,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(
                        child.try_wait().expect("observe fixture process").is_none(),
                        "the named Gateway fixture exited before becoming ready"
                    );
                    assert!(
                        Instant::now() < deadline,
                        "the named Gateway fixture did not become ready"
                    );
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(error) => panic!("accept fixture control connection: {error}"),
            }
        };
        let mut port = [0_u8; 2];
        stream
            .read_exact(&mut port)
            .expect("read fixture Gateway port");
        Self {
            child,
            port: u16::from_be_bytes(port),
            control: stream,
            _directory: directory,
        }
    }

    pub(crate) const fn port(&self) -> u16 {
        self.port
    }

    pub(crate) fn validate(
        &self,
        api_key: &str,
        epoch: u64,
        started_at: &str,
    ) -> ValidatedConnection {
        ValidatedConnection::validate(self.connection_file(api_key, epoch, started_at))
            .expect("the named local Gateway validates")
    }

    pub(crate) fn connection_file(
        &self,
        api_key: &str,
        epoch: u64,
        started_at: &str,
    ) -> ConnectionFile {
        ConnectionFile {
            port: self.port,
            api_key: api_key.to_owned(),
            pid: self.child.id(),
            epoch,
            version: "test".to_owned(),
            started_at: started_at.to_owned(),
        }
    }

    pub(crate) fn received_shutdown(&mut self, timeout: Duration) -> bool {
        self.control
            .set_read_timeout(Some(timeout))
            .expect("set fixture control timeout");
        let mut marker = [0_u8; 1];
        match self.control.read_exact(&mut marker) {
            Ok(()) => {
                assert_eq!(marker, [1], "the fixture reports only shutdown requests");
                true
            }
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                false
            }
            Err(error) => panic!("read fixture shutdown marker: {error}"),
        }
    }
}

impl Drop for ValidatedGateway {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
#[ignore = "runs only as a named child process"]
fn validated_gateway_fixture_process() {
    let Ok(control_address) = std::env::var(CONTROL_ADDRESS_ENV) else {
        return;
    };
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
        while let Some(shutdown) = answer_request(&mut stream, &expected_key) {
            if shutdown {
                control
                    .write_all(&[1])
                    .expect("report the accepted shutdown request");
            }
        }
    }
}

fn answer_request(stream: &mut TcpStream, expected_key: &str) -> Option<bool> {
    let mut buffer = [0_u8; 4096];
    let Ok(read) = stream.read(&mut buffer) else {
        return None;
    };
    if read == 0 {
        return None;
    }
    let request = String::from_utf8_lossy(&buffer[..read]);
    let accepted = request.starts_with("GET /health ")
        || request.contains(&format!("Authorization: Bearer {expected_key}\r\n"));
    let response = if accepted {
        "HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}"
    } else {
        "HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\n\r\n"
    };
    stream
        .write_all(response.as_bytes())
        .is_ok()
        .then(|| accepted && request.starts_with("POST /shutdown "))
}
